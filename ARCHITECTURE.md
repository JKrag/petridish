# petridish — Architecture & Findings

The authoritative reference for what any implementation must honour: empirical findings
about the real environment, the architecture and its single-writer invariant, the
`projects.json` wire schema, the discovery/authorship-filter design, and deferred work.

Everything here is now Rust — the scanner (`swab/`), the dashboard (`petri/`), the shared
schema (`petridish-core/`) and the installer (`petridish-cli/`). The Python original this
document was first written alongside was deleted in ADR-0004; the reasoning survived the
implementation, which is why this file is language-neutral in tone.

Section numbers are stable identifiers, not just structure: source files cite them
(`swab/src/discovery.rs`'s doc comments reference §2, `petridish-cli` references §8.3's
D-numbers). Renumbering silently breaks those citations.

For current stack, invariants and testing conventions, see `CLAUDE.md`.

---

## 0. Findings that changed the design

These were verified empirically on a real machine. They are not assumptions, and they
don't depend on what language reads `~/.claude/projects/` or `workspaceStorage/` — any
future sensor implementation hits the same shapes.

| # | Finding | Consequence |
|---|---------|-------------|
| F1 | `~/.claude/projects/<slug>/<session>.jsonl` already contains `cwd`, `sessionId`, `gitBranch`, `timestamp`, `version` on most lines. | Claude Code sensing needs **no hook to function**. Hooks are a latency optimisation, not the mechanism. |
| F2 | The directory slug (`-Users-jankrag-repos-JKrag-project-radar`) is **not reversibly decodable** — `/` and `-` both map to `-`. | Never parse the dirname for a path. Read `cwd` from a JSONL line. |
| F3 | The last line of a transcript does **not** reliably discriminate "agent working" from "session idle". Observed trailing types across live sessions: `user`, `system`, `last-prompt`. | No clean state machine from transcripts. Liveness = file mtime recency (+ hook events for precision). Documented limitation, not papered over. |
| F4 | `~/.claude/settings.json` typically has other hook consumers already stacked on `Notification`/`PostToolUse` (this machine has three: pixtuoid, statusbar, notchbar). | `swab-hook` must be trivially fast and must **never** write `projects.json`. See §3. |
| F5 | `~/.copilot/` does not exist on a machine without the standalone Copilot CLI installed. | VS Code's own Copilot integration (F6) is the real target, not a hypothetical CLI. |
| F6 | VS Code Copilot **is** attributable: most `workspaceStorage/<hash>/` dirs contain `chatSessions/`, and the sibling `workspace.json` holds `{"folder": "file:///Users/.../project-radar"}`. | This is the direct analogue of `~/.claude/projects`. |
| F7 | `git log -1 --author=<re>` short-circuits fast and returns *when you last touched it*, strictly more useful than a commit count. | Authorship is used as a **recency signal**, not just a clone filter. See §2. |
| F8 | `lsof -a -p <pid> -d cwd` is fast but process-listing tools are often sandbox-restricted, and for Claude Code the cwd is already free from F1. | Process scanning stays out of core. Deferred (§5). |
| F9 | `cwd` **varies within a single transcript** — a session's cwd typically moves into subdirectories of one repo as work continues (`fastfood-filter` → `fastfood-filter/apps/extension`). | Two consequences: (a) read `cwd` from the **last** parseable line, not the first; (b) every raw cwd must be resolved up to its enclosing project root, or one monorepo session shatters into phantom projects. |

---

## 1. Architecture

```text
  ~/.claude/projects/*/*.jsonl ──┐   (F1: cwd + session_id + mtime)
  VS Code workspaceStorage/* ────┤   (F6: workspace.json folder URI + chatSessions mtime)
  ~/.petridish/events.ndjson ────┤   (append-only, written by swab-hook)
  configured roots + git ────────┘
                                 │
                                 ▼
                   swab scan  (launchd timer, ~60s)
                   sole writer, atomic temp+rename
                                 │
                                 ▼
              ~/.petridish/projects.json  (read-only to all frontends)
```

**Single-writer invariant.** With multiple hook consumers typically already installed (F4)
and a launchd timer also writing, an unguarded read-modify-write on one JSON file has no
locking and the corruption is silent. The hook only ever does one `O_APPEND` write of a
single sub-4KB line to `events.ndjson` (atomic on macOS); the daemon is the only process
that ever opens `projects.json` for writing. **Never reintroduce hook writes to
`projects.json`**, in any language.

---

## 2. Discovery & the authorship filter

Project candidates come from three sources, unioned and de-duplicated by resolved realpath:

1. **Configured roots** — crawled, default `["~/repos", "~/learning"]`.
2. **Agent history** — every distinct `cwd` in `~/.claude/projects/*/*.jsonl` and every
   `folder` in VS Code `workspace.json`. Zero-config, and catches projects in "weird"
   locations automatically.
3. **Manual extras** — `extra_paths` in config, for anything the other two miss.

**Crawl rules:** max depth 4; stop descending as soon as a `.git` is found (do not recurse
into submodules); hard-skip `node_modules`, `.worktrees`, `vendor`, `.venv`, `venv`,
`target`, `dist`, `build`, `.next`, `Library`, `.Trash`.

**Authorship filter (refined per F7).** Many roots contain clones the user never worked
on. For each repo compute:

```
mine_last_commit_at = git log -1 --format=%cI --author=<regex> --since=<horizon> HEAD
```

Default regex `Jan.*Krag`, configurable as `author_patterns` (list, matched against
`--author`, which already searches name *and* email). `--since` defaults to `3 years` — it
bounds the history walk so a large clone can't stall a tick.

*Known consequence of the `--since` bound:* a repo genuinely authored more than 3 years ago
reads as never-authored and is flagged `foreign`. Acceptable — such a repo buckets as
`cold` regardless — but deliberate, not an oversight. Raise `author_since` to attribute
deeper back-catalogue.

This value is used two ways:

- **Filter:** repos with no authored commit *and* no agent history *and* a clean tree are
  classified `foreign` and hidden by default (`swab list --all` shows them). Never deleted
  from the JSON — hiding is a frontend concern.
- **Recency:** `mine_last_commit_at` is preferred over `last_commit_at` when bucketing (§4).
  A repo where a bot or teammate pushed yesterday is not *your* active project.

Never let the filter exclude a repo that has agent history or uncommitted changes — those
are positive evidence of involvement that predates or postdates any commit.

---

## 3. `swab-hook` (latency path, optional)

The daemon works correctly without it (F1); the hook exists only to make liveness
sub-second instead of sub-minute. Constraints, given F4:

- Single `O_APPEND` write of one JSON line to `~/.petridish/events.ndjson`, then exit.
- **Never fails loudly.** A crashing hook in a chain that also feeds other consumers
  (statusbar, notch indicators, etc.) is unacceptable — always exit 0.
- Minimal startup cost; target well under the hook-timeout budget of whatever tool invokes
  it (Claude Code, in this repo's case).
- Reads the hook event JSON from stdin; extracts `cwd`, `session_id`, `hook_event_name`.
  Writes the **raw** `cwd` — resolving it to a project root is the daemon's job, not the
  hot path's.
- Installer must append to existing hook arrays, never replace them (F4), and must back up
  `~/.claude/settings.json` first.

**Ownership marker.** Every hook entry the installer adds carries the literal trailing
comment `# petridish` in its `command` string — the same convention other hook consumers on
this machine use. That marker is the shared definition of "our entry" for three contracts:
`swab doctor`'s "hook installed" check, the installer's idempotency check, and
`--uninstall`'s "removes only what it added". Match on the marker, never on the mere
presence of `swab-hook`, and never rewrite an entry that lacks it.

Events are compacted by the daemon: on each tick, read `events.ndjson`, fold into state,
truncate the file. Capped defensively (5MB) in case the daemon isn't running.

---

## 4. Frozen schema (`projects.json`)

**Frozen.** Every reader and writer, in any language, codes against this exact shape.

```jsonc
{
  "schema_version": 1,
  "updated_at": "2026-08-05T22:45:00Z",
  "scan_duration_ms": 412,
  "projects": [
    {
      "id": "sha1-of-realpath-first-12",       // path-derived, NOT stable across moves:
                                               // a relocated project appears as a new entry.
                                               // Frontends must not treat this as a durable key.
      "name": "project-radar",
      "path": "/Users/jankrag/repos/JKrag/project-radar",
      "category": "JKrag",                     // parent dir name; overridable in config
      "parent_path": null,                     // resolved path of the containing project if this is a .worktrees/<name> child; null otherwise
      "is_foreign": false,                     // §2 authorship filter
      "git": {
        "is_repo": true,
        "branch": "master",
        "is_dirty": true,
        "uncommitted_files": 4,
        "last_commit_at": "2026-08-04T14:10:00Z",
        "mine_last_commit_at": "2026-08-04T14:10:00Z",   // null if never authored
        "github_url": "https://github.com/user/project-radar"  // null if no remote
      },
      "agent": {
        "state": "working",                    // working | recent | idle
        "active_agent": "claude-code",         // claude-code | copilot | null
        "last_event": "PreToolUse",            // null when derived from mtime only
        "last_event_at": "2026-08-05T22:44:12Z",
        "session_id": "6f4a8f6e-…",            // resumable via `claude --resume`
        "waiting_since": null                  // MECH-5: set while the agent is blocked on a human; null otherwise
      },
      "last_activity_at": "2026-08-05T22:44:12Z",  // max of all signals; drives bucket
      "status_bucket": "active"                // active | in_flight | stale | cold
    }
  ]
}
```

**`agent.state`** (F3 — mtime-derived, no reliable transcript state machine):
`working` = event or transcript mtime < 90s · `recent` = < 30min · `idle` = older.

**`agent.waiting_since`** (MECH-5) is the one agent fact that is *observed* rather than
inferred from silence, and it exists because the three states above cannot tell a blocked run
from a busy one: both produce no events, so a run held at a permission prompt decays
`working → recent → idle` exactly like one that finished. Claude Code's `Notification` and
`PermissionRequest` hooks fire precisely when a human is needed, so `swab-hook` is registered
on them (and only them — they are rare, unlike `PostToolUse`); `PreToolUse`/`Stop` clear the
latch, and a 3h backstop releases one whose clearing event never arrives. A project with a
live latch buckets `active` regardless of age. It is an optional **field** rather than a
fourth `agent.state` value so that a reader which predates it skips it instead of failing to
parse. No equivalent signal exists for copilot, whose rows stay blank rather than false.

**`status_bucket`** from `last_activity_at` = max(`mine_last_commit_at`, `agent.last_event_at`,
newest mtime among uncommitted files): `active` < 48h · `in_flight` < 14d · `stale` < 60d ·
`cold` beyond. Thresholds live in config, not in code.

**Writer contract:** serialise to `projects.json.tmp` in the same directory, atomic rename
onto the target. Readers therefore never observe a partial file and need no lock.

### 4.1 `AgentSignal` — the internal sensor contract

Not part of `projects.json`; it's the shape every sensor produces and the aggregator
consumes. Illustrated below as field/type pairs, not tied to any language's syntax —
`swab`'s Rust sensors implement this same shape as a struct; a future reimplementation
in any other language should too.

```
AgentSignal:
    root: string              # resolved PROJECT ROOT, already collapsed to its repo root (F9)
    at: datetime (tz-aware UTC)  # the activity timestamp
    agent: string              # "claude-code" | "copilot"
    session_id: string | null  # resumable id where the source provides one
    event: string | null       # hook event name; null when derived from mtime alone
    raw_cwd: string | null     # pre-resolution cwd, for debugging monorepo attribution
```

Sensors return one signal per resolved root (the newest `at` wins within a sensor); the
aggregator merges across sensors by the same rule.

---

## 5. Deferred (explicitly out of scope for now)

Raycast extension is built (`raycast/`, TypeScript, unaffected by any of the Python→Rust
work — see its own `README.md`). Still deferred: FastAPI/web UI · `ps`/`lsof` process
sensing for non-Claude CLIs (F8) · standalone Copilot CLI support if `~/.copilot/` ever
appears for real (F5) · multi-root VS Code workspaces · resume-session from `petri`
(Raycast has it; `petri` does not — see `SURF-3` in `petri/IDEAS.md`).

**No longer deferred:** open-in-editor and open-GitHub actions from `petri` landed in
slice 1 (`petri/SPEC.md` §5.1), along with a git-history action and a tool picker. An
earlier version of this line said `petri` "v1 stays read-only intentionally" — that
claim is retired as written, though the invariant underneath it is untouched: `petri`
still never writes `projects.json`, and handing the terminal to another program is not
the same thing as becoming a writer.

**Superseded:** the original plan's D6 ("`petri` uses stdlib `curses`, not Textual", chosen
to keep the Python build's zero-runtime-deps constraint) is moot — `petri` is **Rust +
ratatui**, its own workspace crate (ADR-0002), spec in `petri/SPEC.md`. The stdlib-only rule
was always specific to the Python read-side, which no longer exists (ADR-0004).

---

## 6. `petri` dashboard

**Superseded — see `petri/SPEC.md`.**

This section used to carry a behaviour spec "for the next port", written before the
dashboard was built. It described the since-deleted Python/curses build incompletely — it
omitted the quota bars, density switching, agent glyphs, silence countdown, worktree tree
rollup and the Dashboard/Browser split that all actually shipped — and the Rust build
deliberately changed some of what it did describe. `petri/SPEC.md` is authoritative.

Terminology (**Dashboard**, **Browser**, **state file** vs **preferences file**) is defined
in `CONTEXT.md`. Decisions: ADR-0002 (crate layout), ADR-0003 (verification), ADR-0001
(worktree rollup).

---

## 7. Raycast extension (`integrations/raycast/`)

Separate npm project, its own toolchain, already built and functioning — see
`integrations/raycast/README.md`. Read-only consumer of `projects.json`, same single-writer
invariant as every other frontend, and it never shells out to `swab`. It now runs in CI
(tsc, tests, lint), which it never did before: it mirrors the wire schema in hand-written
TypeScript, and that is exactly the kind of thing that rots silently when nothing checks
it.

**Still deferred:** a second command mirroring `swab path <query>` (open the best-matching
project directly, no list view). Store publishing (`ray publish`) — blocked on the
GPL-3.0-or-later vs Raycast Store's MIT requirement (a real decision, not a placeholder,
the moment Store publishing is on the table) and real icon artwork.

---

## 8. Distribution & installer requirements

Originally §7 of the archived design doc. The `D`-numbers below are **frozen identifiers**
cited from real code comments in `petridish-cli` — treat them as stable references, not
prose to renumber. Where the world moved underneath a requirement, the requirement is
reinterpreted *in place* rather than dropped or renumbered.

### 8.1 Install model

**Superseded twice, and worth knowing both times.** This first described an all-Python
install; then a two-toolchain one, after `swab`/`swab-hook` became Rust binaries while the
read-side stayed Python. Neither is true now. ADR-0004 removed Python entirely, so there
is one toolchain and one install path.

The primary channel is a Homebrew tap:

```sh
brew install jkrag/tap/petridish
petridish install
```

`brew install` places four binaries — `petridish`, `swab`, `swab-hook`, `petri` — on the
user's `PATH`. `petridish install` is the separate step that wires the tool into the
machine: the launchd job, the Claude Code hook entries, and the menu-bar plugin.

From a checkout, the equivalent is:

```sh
cargo install --path petridish-cli --locked
cargo install --path swab --locked
cargo install --path petri --locked
petridish install
```

`--locked` is load-bearing, not decoration: `cargo install` otherwise ignores `Cargo.lock`
and re-resolves, and a yanked transitive `gix` dependency (`bisync`) makes that resolution
fail outright. `README.md` carries the same warning where a new reader will meet it.

**Why the daemon is not a Homebrew `service` block.** Homebrew formulae can manage a
launchd job directly, which would shorten the path for brew users. Rejected: everyone
installing another way would then have no launchd path at all, and D1/D2 below would hold
for one install route and not the other. One install story that every user exercises is
worth more than a marginally shorter one for some.

**Development installs use the same mechanism as end-user installs.** That is not just
convenience — it means the launchd plist, the hook entries and `doctor` are exercised
against the *real production layout* during development, rather than a dev-only
arrangement that would need re-testing after release.

Rejected alternatives:

| Approach | Why rejected |
| --- | --- |
| Symlink into `/opt/homebrew/bin` by hand | Homebrew-managed directory; `brew doctor` flags foreign files and brew operations can clobber them. |
| Shell alias in `.zshrc` | **Actively broken.** `launchd` jobs, Claude Code hooks and xbar plugins all run non-interactively and never source shell rc files — the daemon and hook would silently fail while working fine in a terminal. |

### 8.2 Release channels

1. **Homebrew tap — primary.** The tool is macOS-only by nature (launchd, `~/Library`), so
   brew is the natural fit for discoverability and upgrades. A personal tap
   (`brew install jkrag/tap/petridish`) rather than `homebrew-core`, which imposes
   notability requirements and ongoing maintenance obligations.
2. **Shell installer — secondary.** A `curl | sh` script for people who do not use brew.
3. **`cargo install` — for Rust users**, from crates.io once published, or from a git
   checkout at any time.

**PyPI is no longer a channel.** It was named the primary, source-of-truth channel here
when the package was Python; there is nothing left to publish there (ADR-0004).

**A note on the crates.io name.** `petridish` is taken by an unrelated, actively-published
project-scaffolding tool, as are the bare `petri` and `swab`. The crate therefore publishes
as `petri-dish` (free, and normalising to `petri_dish`, which does not collide) while
shipping a binary named `petridish`. Crate names and binary names are independent, so
nothing a user sees changes. See ADR-0004.

### 8.3 Design requirements this imposes

- **D1 — Discover the binary path at install time; never hardcode it.** The installer
  resolves each binary on `PATH` and writes the **absolute** result into both the launchd
  plist and `~/.claude/settings.json`. A `cargo install` user has `~/.cargo/bin/swab-hook`;
  a Homebrew user has `/opt/homebrew/bin/swab-hook`. Assuming either breaks the other.
  **Implemented:** `petridish-cli`'s `paths::resolve_binary_in`.

  *Absolutise, but do not canonicalise.* Under Homebrew, `/opt/homebrew/bin/swab` is a
  symlink into a version-stamped Cellar directory; resolving it would bake that version
  into the plist and the next `brew upgrade` would leave launchd pointing at a path that no
  longer exists. `petridish doctor` checks specifically for this — it reads the program
  path back out of the installed plist and verifies it still exists.
- **D2 — Assume no `PATH` in non-interactive contexts.** `launchd` does not inherit the
  user's shell environment; neither do Claude Code hooks; neither does xbar, which is
  launched by the GUI session. Every command string written to disk must be absolute. This
  is why the generated xbar wrapper embeds the binary path rather than calling `petridish`
  by name — a bare invocation works when tested in a terminal and shows nothing in the menu
  bar.
- **D3 — Keep the installer's dependency tree small.** *Reinterpreted:* this originally
  read "runtime dependencies stay at zero for `src/petridish/`", justified by Homebrew
  Python formulae needing a vendored `resource` block per transitive dependency. Both the
  subject and that argument are gone. The requirement survives as a constraint on
  `petridish-cli`, the crate a user runs first and the one that edits files it does not
  own: `clap`, `serde_json`, `chrono`, `petridish-core`. No async runtime, no HTTP client,
  no plist library. PATH lookup, the scratch-directory test helper and the `getuid` call
  are each a few lines of `std` rather than a dependency, deliberately. `swab` and `petri`
  were never bound by this.
- **D4 — The installer is idempotent, reversible, and marks its own work.** It backs up
  `~/.claude/settings.json` before editing (once — a second install must not overwrite a
  clean pre-install snapshot with a dirty one), *appends* to existing hook arrays rather
  than replacing them (real users have other hook consumers), tags every entry it adds with
  the literal marker `# petridish`, and ships an `uninstall` that removes only marked
  entries. Uninstall never restores the backup wholesale: that would silently discard every
  unrelated edit made since.

  *Idempotent per event, not per file.* A whole-file marker check reports "already
  installed" on any machine set up before an event joined `HOOK_EVENTS` — which is every
  machine predating the MECH-5 pair — stranding the new events forever and leaving the
  feature dead exactly where it was supposed to work. **Implemented:**
  `petridish-cli`'s `settings::add_hook_entries`.
- **D5 — Declare macOS-only.** Fail early with a clear message on other platforms rather
  than half-installing. **Implemented:** `petridish-cli`'s `paths::check_platform`, called
  before anything is written, since launchd is where half-installing would actually happen.
  *Reinterpreted:* the original implementation was a pyproject classifier plus
  `installer.py`'s `sys.platform` check, both gone.

  Note that `check_platform` takes the OS name as a *parameter* rather than being a
  `#[cfg(target_os)]` gate. CI compiles this crate on Linux to verify the rest of the
  workspace is not macOS-bound, and a compile-time gate would make its tests unrunnable
  there. The scanner is deliberately *not* platform-gated at all: its sensors degrade to
  `null`/empty per CLAUDE.md invariant 5 (the Copilot sensor finding no `workspaceStorage/`
  on Linux is that invariant working, not a bug), so gating the whole CLI would reject
  strictly more than necessary.
- **D6 — Respect user-data separation.** User state (`config.toml`, `projects.json`,
  `events.ndjson`, the settings backup) lives in `~/.petridish/` and must survive
  uninstall/reinstall untouched. Only the binaries are managed by the package manager.

### 8.4 Naming — resolved

`project-radar` was a working title, replaced (2026-08-06) with **`petridish`** — the
filesystem-as-Petri-dish metaphor for dozens of small AI-assisted experiments growing (or
sitting dormant) across the machine. Final mapping:

- Project, Homebrew formula, user-data directory: `petridish`, `~/.petridish/`
- Installer / health-check / menu-bar binary: `petridish`
- Scanner: `swab` — "swabbing" the filesystem to see what is alive. Replaced the working
  name `radar`.
- Hook: `swab-hook`. Replaced the working name `radar-hook`.
- Interactive dashboard: `petri` (Rust/ratatui; `petri/SPEC.md`).
- launchd label: `com.petridish.daemon`
- Hook marker written into `settings.json`: `# petridish` — load-bearing, since both
  `doctor` commands and `uninstall` identify their own entries by matching it.

**Registry names, and why one of them differs.** The bare `petri` was taken on PyPI when
the name was first chosen, which is why `petridish` became the distribution name; PyPI is
no longer a channel (§8.2). On crates.io, `petridish`, `petri` and `swab` are *all* taken
by unrelated live crates. The installer crate therefore publishes as **`petri-dish`** —
free, and normalising to `petri_dish`, which does not collide with `petridish` — while
still shipping a binary named `petridish`. Crate names and binary names are independent, so
this is invisible to users; only `cargo install` ever names the crate. See ADR-0004 for the
alternatives weighed, including renaming the project outright.
