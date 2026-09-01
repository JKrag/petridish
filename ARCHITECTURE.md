# petridish — Architecture & Findings

Successor to `docs/archive/IMPLEMENTATION_PLAN.md` for everything in that document that's
still true regardless of implementation language. That plan was written for an all-Python
build; the scanner is now Rust (`swab/`) and `petri` (the TUI) is slated for a Go or
Rust rewrite next. This document captures what any implementation — in any language — must
honor: empirical findings about the real environment, the architecture and its
single-writer invariant, the `projects.json` wire schema, the discovery/authorship-filter
design, and forward-looking deferred work. Section numbers below intentionally mirror the
archived plan's where the content survived unchanged, since several source files still
reference specific sections by number (e.g. `swab/src/discovery.rs`'s doc comments).

For current stack/invariants/testing conventions, see `CLAUDE.md`. For the original
all-Python build history, see `docs/archive/IMPLEMENTATION_PLAN.md`.

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
        "session_id": "6f4a8f6e-…"             // resumable via `claude --resume`
      },
      "last_activity_at": "2026-08-05T22:44:12Z",  // max of all signals; drives bucket
      "status_bucket": "active"                // active | in_flight | stale | cold
    }
  ]
}
```

**`agent.state`** (F3 — mtime-derived, no reliable transcript state machine):
`working` = event or transcript mtime < 90s · `recent` = < 30min · `idle` = older.

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
appears for real (F5) · multi-root VS Code workspaces · resume-session / open-in-editor /
open-GitHub actions from `petri` specifically (Raycast already has these; `petri` v1 stays
read-only intentionally, see `petri/SPEC.md`).

**Superseded:** the archived plan's D6 ("`petri` uses stdlib `curses`, not Textual", chosen
to keep the Python build's zero-runtime-deps constraint) is moot now that `petri` itself is
being reimplemented off Python. Rendering engine is now decided: **Rust + ratatui**, its own
workspace crate (ADR-0002), spec in `petri/SPEC.md`. The stdlib-only rule was always
`src/petridish/`-specific and never bound the Rust side.

---

## 6. `petri` dashboard

**Superseded — see `petri/SPEC.md`.**

This section used to carry a behavior spec "for the next port". It described the
Python/curses build incompletely (it omitted the quota bars, density switching,
agent glyphs, silence countdown, worktree tree rollup and the Dashboard/Browser
split that all actually shipped), and the Rust reimplementation deliberately changes
some of what it did describe. `petri/SPEC.md` is the authoritative spec; the parity
baseline is `petripy`'s running code, not any prose here.

Terminology (**Dashboard**, **Browser**, **petripy**, **state file** vs
**preferences file**) is defined in `CONTEXT.md`. Decisions: ADR-0002 (crate
layout), ADR-0003 (verification), ADR-0001 (worktree rollup, still current).

---

## 7. Raycast extension (`raycast/`)

Separate npm project, its own toolchain, already built and functioning — see
`raycast/README.md` for current details. Read-only consumer of `projects.json`, same
single-writer invariant as every other frontend. Full historical build notes (toolchain
quirks, decisions made without the user at the time, what's still placeholder like the
icon) are preserved in `docs/archive/IMPLEMENTATION_PLAN.md` §9 if needed, but nothing
there is stale — Raycast wasn't touched by the Python→Rust scanner work and isn't affected
by the `petri` reimplementation either.

**Still deferred:** a second command mirroring `swab path <query>` (open the best-matching
project directly, no list view). Store publishing (`ray publish`) — blocked on the
GPL-3.0-or-later vs Raycast Store's MIT requirement (a real decision, not a placeholder,
the moment Store publishing is on the table) and real icon artwork.

---

## 8. Distribution & installer requirements

Extracted verbatim from `docs/archive/DESIGN.md` §7 when that doc was archived — this
section, unlike the rest of that doc, was and is live: `src/petridish/installer.py`
cites `D1`–`D6` and `§8.1` by number in real docstrings/comments. Treat the `D`-numbers as
frozen identifiers, not prose to rephrase — renumbering them silently breaks those
citations. `installer.py` is now built (this described it before it existed); the
requirements still describe why it's built the way it is.

### 8.1 Install model

The tool ships as a **Python package with two console scripts** (`swab`,
`swab-hook`), installed into an **isolated virtualenv** with shims placed on the
user's `PATH`. It is deliberately *not* a `pip install` into the system or user
site-packages: it is an application, not a library, so `uv tool` / `pipx` semantics
are the right ones.

**Development installs use the same mechanism as end-user installs:**

```sh
uv tool install --editable ~/repos/JKrag/project-radar   # dev (repo dir name unchanged)
uv tool install <package-name>                           # end user (post-PyPI)
pipx install <package-name>                              # end user, equivalent
```

Both produce shims in `~/.local/bin`. This is not just convenience — it means the
launchd plist, the Claude hook entry, and `swab doctor` are all exercised against
the **real production layout** during development, rather than a dev-only
arrangement that would need re-testing after release.

Rejected alternatives:

| Approach | Why rejected |
| --- | --- |
| Symlink into `/opt/homebrew/bin` | Homebrew-managed directory; `brew doctor` flags foreign files and brew operations can clobber them. |
| Shell alias in `.zshrc` | **Actively broken.** `launchd` jobs and Claude Code hooks run non-interactively and never source shell rc files — the daemon and hook would silently fail while working in an interactive terminal. |
| `pip install --user` | PEP 668 friction on managed Pythons; pollutes user site-packages; no isolation. |

### 8.2 Release channels

1. **PyPI — primary, source of truth.** Enables `uv tool install` / `pipx install`
   immediately and is what any other channel wraps.
2. **Homebrew tap — secondary, optional.** Because the tool is macOS-only by nature
   (launchd, `~/Library` paths), brew is a natural fit for discoverability. Start
   with a personal tap (`brew install <user>/tap/<name>`) rather than
   `homebrew-core`, which imposes notability requirements (stars/forks, release
   history) and ongoing maintenance obligations.

**The zero-dependency rule is a distribution asset, not just a build constraint.**
Homebrew Python formulae normally require a vendored `resource` block per
transitive dependency — often dozens, each needing bumps. A stdlib-only package
needs none, making the formula roughly fifteen lines. This is a strong reason to
keep `src/petridish/`'s runtime dependencies at zero permanently, beyond the
original testability rationale (CLAUDE.md).

### 8.3 Design requirements this imposes

- **D1 — Discover the binary path at install time; never hardcode it.** The
  installer must resolve `command -v swab-hook` and write the **absolute** result
  into both the launchd plist and `~/.claude/settings.json`. A `uv tool` user has
  `~/.local/bin/swab-hook`; a Homebrew user has `/opt/homebrew/bin/swab-hook`.
  Assuming either breaks the other.
- **D2 — Assume no `PATH` in non-interactive contexts.** `launchd` does not inherit
  the user's shell environment, and neither do Claude Code hooks. Every command
  string written to disk must be absolute.
- **D3 — Runtime dependencies stay at zero** for `src/petridish/`. See §8.2. Adding
  one there is a distribution decision, not just an implementation one. (Does not
  apply to the Rust crates — CLAUDE.md.)
- **D4 — The installer is idempotent, reversible, and marks its own work.** It must
  back up `~/.claude/settings.json` before editing, *append* to existing hook arrays
  (never replace them — real users have other hook consumers), tag every entry it
  adds with a literal marker comment, and ship an `--uninstall` that removes only
  marked entries.
- **D5 — Declare macOS-only.** Add the appropriate classifier and fail early with a
  clear message on other platforms rather than half-installing. **Implemented:**
  `Operating System :: MacOS :: MacOS X` classifier in `pyproject.toml`, plus
  `installer.py`'s `check_platform()` (`sys.platform != "darwin"` raises before touching
  anything) — that's where "half-installing" would actually happen, since launchd is the
  macOS-only surface. The CLI itself (`swab scan`/`list`) is deliberately *not*
  platform-gated: its sensors already degrade to `null`/empty per CLAUDE.md invariant 5
  (e.g. the Copilot sensor finding no `workspaceStorage/` on Linux is that invariant
  working, not a bug), so gating the whole CLI would reject strictly more than necessary.
- **D6 — Respect user-data separation.** Code lives in the tool's venv; user state
  (`config.toml`, `projects.json`, `events.ndjson`) lives in `~/.petridish/` and
  must survive uninstall/reinstall untouched.

### 8.4 Naming — resolved

`project-radar` was a working title, replaced (2026-08-06) with **`petridish`** —
the filesystem-as-Petri-dish metaphor for dozens of small AI-assisted experiments
growing (or sitting dormant) across the machine. The bare name `petri` was already
taken on PyPI at the time; `petridish` was checked and free, so it is the
distribution name. `petri` was kept in reserve for the dashboard frontend and is
now that frontend's actual name (`petri/SPEC.md`), not used for anything published
to PyPI. Final mapping:

- PyPI distribution / import package: `petridish`
- CLI console script (scan/list/path/doctor): `swab` — "swabbing" the filesystem
  to see what's alive. Replaces the working name `radar`.
- Hook console script: `swab-hook`. Replaces the working name `radar-hook`.
- Interactive dashboard: `petri` (Rust/ratatui; `petri/SPEC.md`).
- User-data directory: `~/.petridish/`
- launchd label: `com.petridish.daemon`
- Hook marker string written into `settings.json`: `# petridish` — load-bearing,
  since `swab doctor` and `--uninstall` identify their own entries by matching it.
