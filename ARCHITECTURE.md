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
read-only intentionally, see §6).

**Superseded:** the archived plan's D6 ("`petri` uses stdlib `curses`, not Textual", chosen
to keep the Python build's zero-runtime-deps constraint) is moot now that `petri` itself is
being ported off Python — a Go/Bubbletea or Rust/ratatui rewrite has its own dependency
norms and isn't bound by that constraint. Revisit rendering-engine choice fresh in whichever
language is picked.

---

## 6. `petri` dashboard — behavior spec (for the next port)

The following behavior contract is what any `petri` reimplementation (Go/Bubbletea,
Rust/ratatui, or otherwise) needs to replicate — extracted from the original Python/curses
build's spec, described here independent of that language's dataclasses/curses APIs.
Read-only consumer of `~/.petridish/projects.json`; same single-writer invariant as every
other frontend (§1) — never writes it. No launch/open actions in v1 (Raycast already covers
those, §5).

**State/rendering split.** Keep the "pure state, dumb renderer" split the Python build used
— pure functions over the `Radar`/`Project` shapes (§4) and plain data (no direct
UI-framework calls), so the actual behavior is testable without driving a real terminal:

- **Grouping:** buckets in the fixed display order `active, in_flight, stale, cold`.
  Excludes `is_foreign` projects (no `--all`-equivalent toggle in `petri` itself — that's
  `swab list --all`'s job).
- **Filtering:** case-insensitive substring match against `Project.name`. Empty query
  returns the input unchanged.
- **Row formatting:** same four columns and same agent-label / dirty-marker logic as
  `swab list`'s table (name, agent, branch, dirty marker). Don't duplicate that logic
  independently in two places if avoidable — share it with (or mirror it exactly against)
  `swab`'s `cli.rs`.
- **Detail panel:** path, branch, dirty file count, last commit time (and
  `mine_last_commit_at` if it differs), github url, agent state/active agent/session_id,
  last_activity_at.
- **Selection movement:** moves up/down by a delta, **crossing section boundaries**
  (skipping empty sections), clamped — never wraps — at the very top/bottom of the whole
  list. Selecting nothing (empty filtered set) must be representable and handled without
  crashing. Re-filtering must not crash if the previously-selected project got filtered
  out — selection resets to the first available row.
- **Staleness banner:** if the state file's `updated_at` is older than a threshold
  (24h, matching `swab doctor`'s own freshness check), still render normally but show a
  persistent banner — data degrades visibly, the screen never silently lies about
  freshness.
- **Missing state file:** before entering the interactive screen at all, check the file
  exists; if not, print the same message `swab list`/`swab path` already use
  (`"no state file at {path}; run 'swab scan' first"`) and exit 1 — no blank/broken screen.
- **Auto-poll:** poll the state file's mtime on a short timer (~2–5s) and only re-read +
  re-render when it changed. A plain stat-poll, not a file watcher — no new dependency
  needed for this.
- **Keybindings (v1 baseline):** `/` opens a type-ahead filter that live-filters as you
  type, `Enter`/`Esc` closes it; arrow keys and `j`/`k` move selection; `q` quits. Tab (or
  equivalent) switches between the grouped dashboard and a flat browser view.
- **Resilience:** a too-small or resized terminal must not crash the program — clip or show
  a "resize terminal" message instead.
- **Verification is not just unit tests.** Whatever pure-function tests are feasible, plus
  a mandatory human smoke test in a real terminal before considering this done — confirm
  sections render, filtering works live, selection movement doesn't crash at boundaries,
  and quit is clean. State this explicitly in any handoff rather than reporting green tests
  as "done."

---

## 7. Raycast extension (`raycast/`)

Separate npm project, its own toolchain, already built and functioning — see
`raycast/README.md` for current details. Read-only consumer of `projects.json`, same
single-writer invariant as every other frontend. Full historical build notes (toolchain
quirks, decisions made without the user at the time, what's still placeholder like the
icon) are preserved in `docs/archive/IMPLEMENTATION_PLAN.md` §9 if needed, but nothing
there is stale — Raycast wasn't touched by the Python→Rust scanner work and isn't affected
by any planned `petri` rewrite.

**Still deferred:** a second command mirroring `swab path <query>` (open the best-matching
project directly, no list view). Store publishing (`ray publish`) — blocked on the
GPL-3.0-or-later vs Raycast Store's MIT requirement (a real decision, not a placeholder,
the moment Store publishing is on the table) and real icon artwork.
