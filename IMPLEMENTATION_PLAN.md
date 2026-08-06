# petridish — Implementation Plan

Companion to `DESIGN.md`. This document is the authoritative build spec: it supersedes
`DESIGN.md` where the two disagree, and records *why*. It is written to survive a session
clear — a fresh agent (Sonnet/Haiku, or `/delegate-afk`) should be able to pick up any
single module below without re-reading the conversation that produced it.

**Scope of this plan:** daemon + Claude/Copilot sensing + `swab` CLI.
Raycast extension, TUI, and Web UI are explicitly **out of scope** and get their own plan
once `projects.json` exists with real data in it.

---

## 0. Findings that changed the design

These were verified empirically on this machine (2026-08-05). They are not assumptions.

| # | Finding | Consequence |
|---|---------|-------------|
| F1 | `~/.claude/projects/<slug>/<session>.jsonl` already contains `cwd`, `sessionId`, `gitBranch`, `timestamp`, `version` on most lines. | Claude Code sensing needs **no hook to function**. Hooks become a latency optimisation, not the mechanism. |
| F2 | The directory slug (`-Users-jankrag-repos-JKrag-project-radar`) is **not reversibly decodable** — `/` and `-` both map to `-`. | Never parse the dirname for a path. Read `cwd` from a JSONL line. |
| F3 | The last line of a transcript does **not** reliably discriminate "agent working" from "session idle". Observed trailing types across live sessions: `user`, `system`, `last-prompt`. | No clean state machine from transcripts. Liveness = file mtime recency (+ hook events for precision). Documented limitation, not papered over. |
| F4 | `~/.claude/settings.json` already has three hook consumers stacked on `Notification`/`PostToolUse` (pixtuoid, statusbar, notchbar). | `swab-hook` must be trivially fast and must **never** write `projects.json`. See §3. |
| F5 | `~/.copilot/` **does not exist** on this machine. The DESIGN.md "Copilot CLI" row is unfounded. | Dropped. VS Code Copilot is the real target. |
| F6 | VS Code Copilot **is** attributable: 78 of 91 `workspaceStorage/<hash>/` dirs contain `chatSessions/`, and the sibling `workspace.json` holds `{"folder": "file:///Users/jankrag/repos/JKrag/project-radar"}`. | This is the direct analogue of `~/.claude/projects` the user asked about. Module M5 is implementable, not a spike. |
| F7 | `git log -1 --author=<re>` short-circuits (0.01s) and returns *when you last touched it*, strictly more useful than `shortlog -sn`'s count. | Authorship is used as a **recency signal**, not just a clone filter. See §2. |
| F8 | `lsof -a -p <pid> -d cwd` is fast (32ms) but `ps` is sandbox-restricted, and for Claude Code the cwd is already free from F1. | Process scanning is dropped from core. Deferred to a future optional module. |
| F9 | `cwd` **varies within a single transcript** — measured 1, 2, and 3 distinct values across sample sessions, and the variants are typically *subdirectories of one repo* (`fastfood-filter`, `fastfood-filter/apps/extension`, `fastfood-filter/packages/core`). | Two consequences: (a) read `cwd` from the **last** parseable line, not the first — the first attributes fresh mtime to wherever the session *started*; (b) every raw cwd must be resolved up to its enclosing project root, or one monorepo session shatters into phantom projects. See `resolve_root` in M2. |

---

## 1. Architecture (as revised)

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

**Single-writer invariant.** `DESIGN.md` had `swab-hook` mutating `projects.json` directly.
With three other hook consumers already installed (F4) and a launchd timer also writing,
that is a concurrent read-modify-write on one JSON file with no locking — the corruption
is silent and the failure mode is a wiped board. Revised: the hook only ever does one
`O_APPEND` write of a single sub-4KB line to `events.ndjson` (atomic on macOS), and the
daemon is the only process that ever opens `projects.json` for writing. **Do not
reintroduce hook writes to `projects.json`.**

---

## 2. Discovery & the authorship filter

Project candidates come from three sources, unioned and de-duplicated by resolved realpath:

1. **Configured roots** — crawled, default `["~/repos", "~/learning"]`.
2. **Agent history** — every distinct `cwd` in `~/.claude/projects/*/*.jsonl` and every
   `folder` in VS Code `workspace.json`. Zero-config, and catches projects in "weird"
   locations (`~/.claude/skills/<x>`) automatically.
3. **Manual extras** — `extra_paths` in config, for anything the other two miss.

**Crawl rules:** max depth 4; stop descending as soon as a `.git` is found (do not recurse
into submodules); hard-skip `node_modules`, `.worktrees`, `vendor`, `.venv`, `venv`,
`target`, `dist`, `build`, `.next`, `Library`, `.Trash`.

**Authorship filter (the user's idea, refined per F7).** Many roots contain clones the user
never worked on. For each repo compute:

```
mine_last_commit_at = git log -1 --format=%cI --author=<regex> --since=<horizon> HEAD
```

Default regex `Jan.*Krag`, configurable as `author_patterns` (list, matched against
`--author`, which already searches name *and* email). `--since` defaults to `3 years` — it
bounds the history walk so a large clone can't stall a tick.

*Known consequence of the `--since` bound:* a repo you genuinely authored but last touched
more than 3 years ago reads as never-authored and is flagged `foreign`. This is acceptable
because such a repo would bucket as `cold` regardless, but it is a deliberate tradeoff, not
an oversight — raise `author_since` if you want the deep back-catalogue attributed.

This value is used **two ways**:

- **Filter:** repos with no authored commit *and* no agent history *and* a clean tree are
  classified `foreign` and hidden by default (`swab list --all` shows them). They are not
  deleted from the JSON — hiding is a frontend concern, and a wrong filter that silently
  eats a real project is worse than a slightly noisy list.
- **Recency:** `mine_last_commit_at` is preferred over `last_commit_at` when bucketing
  (§4). A repo where a bot or teammate pushed yesterday is not *your* active project.

Never let the filter exclude a repo that has agent history or uncommitted changes —
those are positive evidence of your involvement that predates or postdates any commit.

---

## 3. `swab-hook` (latency path, optional)

The daemon works correctly without it (F1); the hook exists only to make the 🟢 badge
sub-second instead of sub-minute. Constraints, given F4:

- Single `O_APPEND` write of one JSON line to `~/.petridish/events.ndjson`, then exit.
- **Never fails loudly.** Wrap everything in a bare `except: sys.exit(0)`. A crashing hook
  in a chain that also feeds the user's statusbar and Bartender notch is unacceptable.
- No imports beyond `sys`, `os`, `json`, `time`. Target < 15ms wall.
- Reads the hook event JSON from stdin; extracts `cwd`, `session_id`, `hook_event_name`.
  It writes the **raw** `cwd` — `resolve_root()` is the daemon's job, not the hot path's.
- Installer **must** append to the existing `hooks` arrays, never replace them (F4), and
  must back up `~/.claude/settings.json` first.

**Ownership marker.** Every hook entry the installer adds carries the literal trailing
comment `# petridish` in its `command` string (the same convention notchbar already
uses in this settings file). That marker is the *shared definition of "our entry"* for
three contracts: `swab doctor`'s "hook installed" check, the installer's idempotency
check, and `--uninstall`'s "removes only what it added". Match on the marker, never on the
mere presence of `swab-hook`, and never rewrite an entry that lacks it.

Events are compacted by the daemon: on each tick, read `events.ndjson`, fold into state,
truncate the file. Cap it at 5MB defensively in case the daemon is not running.

---

## 4. Frozen schema (`projects.json`)

**This is module M1 and it is frozen once merged.** Every other module codes against it.

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

**Writer contract:** serialise to `projects.json.tmp` in the same directory, `os.replace()`
onto the target. Readers therefore never observe a partial file and need no lock.

### 4.1 `AgentSignal` — the internal sensor contract

Not part of `projects.json`; it is the shape **M4, M5 and M6 all return** and **M7
consumes**. It is defined in M1 and frozen alongside the schema, so the three sensors can
be built in parallel without inventing three incompatible shapes.

```python
@dataclass(frozen=True)
class AgentSignal:
    root: str              # resolved PROJECT ROOT, already through resolve_root() (F9)
    at: datetime           # tz-aware UTC; the activity timestamp
    agent: str             # "claude-code" | "copilot"
    session_id: str | None # resumable id where the source provides one
    event: str | None      # hook event name; None when derived from mtime alone
    raw_cwd: str | None    # pre-resolution cwd, for debugging monorepo attribution
```

Sensors return `dict[str, AgentSignal]` keyed by `root`, already collapsed to one signal
per root (the newest `at` wins within a sensor). M7 merges across sensors by the same rule.

---

## 5. Modules

Stack: **Python 3.12+, stdlib only** for the daemon (no `watchdog` — the launchd timer
replaces it), `pytest` for tests. Keeping runtime deps at zero is deliberate: it makes each
module independently verifiable by a delegated agent with no environment setup.

Layout: `src/petridish/`, tests in `tests/`, console scripts `swab` and `swab-hook`.

**Dependency graph.** M1 freezes both the JSON schema (§4) and `AgentSignal` (§4.1); M2
owns `resolve_root()`, which M4 and M5 both call (F9). So the fan-out is two waves, not one:

```
                        ┌── M3 git ──────────────────┐
M0 ──> M1 ──> M2 ──> ───┼── M4 claude ───────────────┤
    (schema)  (resolve  ├── M5 copilot ──────────────┼──> M7 ──> M8 ──> M9
              _root)    └── M6 events ───────────────┘   aggregate cli install
```

M3 and M6 don't actually need `resolve_root` and may start alongside M2; M4 and M5 must
wait for it, or be handed the frozen signature
`resolve_root(cwd: str, config: Config) -> Path` and import against it.

Every module below is done when its **Verify** command exits 0. Delegated agents must not
edit another module's files, and must not weaken a test to make it pass.

---

### M0 — Scaffold
**Files:** `pyproject.toml`, `src/petridish/__init__.py`, `src/petridish/config.py`, `tests/conftest.py`, `.gitignore`
**Contract:** `load_config(path=None) -> Config` (frozen dataclass). Reads
`~/.petridish/config.toml` via `tomllib`; every field has a default so a missing file
is valid. Fields: `roots`, `extra_paths`, `author_patterns`, `author_since`,
`ignore_dirs`, `bucket_thresholds`, `category_overrides`, `max_depth`.
`~` and env vars expanded on load. Console scripts `swab`/`swab-hook` declared here.
**Verify:** `pytest tests/test_config.py -q`

### M1 — Schema (freeze first, then parallelise)
**Files:** `src/petridish/schema.py`, `tests/test_schema.py`, `tests/fixtures/projects.golden.json`
**Contract:** dataclasses `GitState`, `AgentState`, `Project`, `Radar` mirroring §4 exactly,
plus `to_dict()`/`from_dict()` round-trip and `write_atomic(radar, path)` implementing the
temp+`os.replace` contract. `schema_version = 1`.
**Verify:** `pytest tests/test_schema.py -q` — must assert round-trip equality against the
golden fixture and that `write_atomic` leaves no `.tmp` behind.

### M2 — Discovery
**Files:** `src/petridish/discovery.py`, `tests/test_discovery.py`
**Contract:** three functions.
- `discover(config) -> list[Path]` — §2 sources 1–3, deduped by `Path.resolve()`.
- `is_foreign(path, config) -> bool` — the authorship filter, including the "never exclude
  if agent history or dirty tree" guard.
- `resolve_root(cwd, config) -> Path` — **required by M4, M5 and M7 (F9).** Walk upward
  from `cwd` to the nearest ancestor containing `.git`, bounded by the configured roots
  (never escape above a root, never return `/` or `$HOME`). Fall back to `cwd` itself when
  no `.git` ancestor is found. This is what keeps `fastfood-filter/packages/core` and
  `fastfood-filter/apps/extension` attributed to the one `fastfood-filter` project instead
  of three. Memoise it — the sensors call it once per transcript file.

**Verify:** `pytest tests/test_discovery.py -q`. Tests build a temp tree with a real repo,
a `node_modules` decoy, a nested repo below a `.git`, and a clone authored only by
`Someone Else <x@y.z>`; assert the decoys are skipped and the clone is flagged foreign.
`resolve_root` must be tested against a monorepo fixture with a package subdir, and
against a path outside all roots.

### M3 — Git scanner
**Files:** `src/petridish/git.py`, `tests/test_git.py`
**Contract:** `scan(path) -> GitState`. Uses `subprocess.run` with a **5s timeout** and
`check=False`; any git failure yields `GitState(is_repo=False)` rather than raising.
Commands: `status --porcelain`, `rev-parse --abbrev-ref HEAD`, `log -1 --format=%cI`,
`log -1 --format=%cI --author=… --since=…`, `remote get-url origin`.
`github_url` must normalise both `git@github.com:u/r.git` and `https://…/r.git` to a
browser URL, and return `None` for non-GitHub remotes.
**Verify:** `pytest tests/test_git.py -q` against repos built by `conftest.py` fixtures
(real `git init` in tmpdir with pinned author/date env — no mocks).
**Edge cases that must be tested:** empty repo with zero commits (this repo was in exactly
that state at plan time), detached HEAD, no remote.

### M4 — Claude Code sensing
**Files:** `src/petridish/sensors/claude.py`, `tests/test_sensor_claude.py`
**Contract:** `scan(claude_dir, config) -> dict[str, AgentSignal]` keyed by resolved root.
Walk `~/.claude/projects/*/*.jsonl`. For each file take mtime, then extract `cwd` and
`sessionId` **from the file's contents, never the dirname** (F2).

Per F9, `cwd` varies within a file, so: take `sessionId` from the first line that has one,
and `cwd` from the **last parseable line** that has one — the mtime you are about to use as
the liveness timestamp describes the session's *current* directory, not its starting one.
Then pass that through `resolve_root()` so monorepo subdirs collapse to one project.

Read the tail efficiently — seek to `max(0, size - 64KB)`, discard the first partial line,
and scan forward; do not parse megabyte transcripts line-by-line from the top. Skip files
whose mtime is older than the `cold` threshold before opening them at all.

Truncated final lines are **normal**, not an error: a live session is being appended to as
you read. Catch `json.JSONDecodeError` on the last line and fall back to the previous one.
**Verify:** `pytest tests/test_sensor_claude.py -q` with a fixture transcript dir covering:
a dash-containing project path (F2), a file whose `cwd` changes mid-session to a subdir
(F9), a half-written trailing line, and a file larger than the 64KB tail window.

### M5 — VS Code Copilot sensing
**Files:** `src/petridish/sensors/copilot.py`, `tests/test_sensor_copilot.py`
**Contract:** `scan(workspace_storage_dir, config) -> dict[str, AgentSignal]` keyed by
resolved root (run the folder path through `resolve_root()` as M4 does). For each
`<hash>/` read `workspace.json` → `folder` (a `file://` URI; parse with `urllib.parse` +
`url2pathname`, do **not** string-slice, paths may contain spaces/`%20`). Signal time =
newest mtime under `<hash>/chatSessions/`. Skip dirs lacking either `workspace.json` or
`chatSessions/` (13 of 91 on this machine). Entries keyed on a `workspace:` URI rather
than `folder` (multi-root workspaces) may be skipped in v1 — log and continue.
**Verify:** `pytest tests/test_sensor_copilot.py -q`

### M6 — Hook + event log
**Files:** `src/petridish/hook.py`, `src/petridish/events.py`, `tests/test_events.py`
**Contract:** `hook.main()` per §3 — stdin JSON in, one appended line out, never raises,
never touches `projects.json`. `events.read_and_compact(path) -> dict[str, AgentSignal]`
folds the log into per-cwd latest state and truncates; tolerates a corrupt/partial line by
skipping it; enforces the 5MB cap.
**Verify:** `pytest tests/test_events.py -q`, including a concurrency test that appends
from 8 processes and asserts no line interleaving/loss.

### M7 — Aggregator (the tick)
**Files:** `src/petridish/scan.py`, `tests/test_scan.py`
**Contract:** `run_scan(config) -> Radar`. Orchestrates M2–M6, merges agent signals
(most-recent wins; `active_agent` = whichever source produced the winning timestamp),
computes `last_activity_at`, `agent.state`, `status_bucket` per §4, writes atomically via
M1. A single failing sensor must degrade that field to `null`, never abort the tick.
**Verify:** `pytest tests/test_scan.py -q` — end-to-end against a synthetic HOME fixture,
asserting the output validates against the M1 schema and that a sensor raising an
exception still produces a complete file.

### M8 — CLI
**Files:** `src/petridish/cli.py`, `tests/test_cli.py`
**Contract:** `argparse`, no deps. Subcommands:
- `swab scan` — run a tick, print a one-line summary
- `swab list [--bucket B] [--all] [--json]` — read cached JSON, human table by default;
  `--all` includes `is_foreign`
- `swab doctor` — verify config, roots exist, hook installed (match the `# petridish`
  marker per §3, not the bare `swab-hook` string), JSON freshness, launchd loaded;
  exit non-zero on any problem
- `swab path <query>` — print the best-matching project path (enables `cd $(swab path x)`)

`list` must **never** trigger a scan (it is the frontend hot path); if the JSON is missing,
say so and point at `swab scan`.
**Verify:** `pytest tests/test_cli.py -q`

**Post-M9 addition:** `swab config` — prints the config file location, every `Config` field
(sourced from `dataclasses.fields(Config)` so it can't drift from the code), and an example.
Added once M9's installer made `~/.petridish/config.toml` a real, present file users would
actually want to look up. The top-level `swab --help` description also names the config path.

### M9 — Install & launchd
**Files:** `install.sh`, `src/petridish/installer.py`, `tests/test_installer.py`,
`resources/com.petridish.daemon.plist`, `README.md`. All settings.json/plist logic lives in
`installer.py` (unit-tested against tmpdir fixtures, no real system mutation from tests);
`install.sh` is a thin wrapper that resolves the correct interpreter (the `uv tool` venv's,
not the ambient `python3`) and execs `python3 -m petridish.installer`.
**Contract:** `StartInterval` 60s, `RunAtLoad`, stdout/stderr to
`~/.petridish/daemon.log`, `ProcessType: Background` so macOS deprioritises it.
Installer: create `~/.petridish/`, write a default `config.toml` **only if absent**,
**back up `~/.claude/settings.json` before touching it**, and append `swab-hook` to
existing hook arrays without disturbing the three existing consumers (F4). Must be
idempotent, and must ship an `--uninstall` that unloads the plist and **structurally removes
only the hook entries it added** — never a verbatim restore from the backup, which would
silently discard any unrelated edit made to `settings.json` between install and uninstall (D4).
**Verify:** `./install.sh && swab scan && swab doctor` exits 0 (`doctor` run right after `install.sh`
races `RunAtLoad`'s async first tick — the state-file check needs a scan to have actually
completed). On a `settings.json` untouched since install,
`./install.sh --uninstall` reproduces the pre-install bytes exactly (tested directly against
`add_hook_entries`/`remove_marker_entries` in `tests/test_installer.py`, not by reading the
backup back). Log rotation: `swab scan` truncates `daemon.log` past 5MB (launchd itself has no
rotation facility).

---

## 6. Delegation notes

- **M1 must be merged before anything else starts**, and it must include *both* the
  `projects.json` schema (§4) and `AgentSignal` (§4.1). Fanning out M4/M5/M6 against an
  undefined `AgentSignal` guarantees three incompatible sensor shapes and a painful M7.
- **M2 must land before M4/M5** because of `resolve_root()`. M3 and M6 can run alongside it.
- The wave-2 modules (M3–M6) touch disjoint files — that's the fan-out for
  `/worktree-provision` or parallel `/delegate-afk` runs.
- M7 is the integration point and should be done by one agent after M2–M6 land.
- Fixtures over mocks: M3's tests should `git init` real repos in tmpdirs, and M4/M5's
  should write real fixture files. Mocked subprocess output would have hidden every one of
  F1–F6.
- Per the engineering-integrity rules: if a module can't be built as specified, escalate
  with the reason — do not narrow the spec, skip the test, or stub the sensor silently.

### M10 — Packaging & release prep
**Files:** `pyproject.toml`, `LICENSE`, `README.md`, `.github/workflows/ci.yml`
**Contract:** everything needed before a first public release. See `DESIGN.md` §7
for the distribution plan and requirements D1–D6 that motivate this.

`pyproject.toml` currently carries only the build minimum. Add:

- `license` (+ a `LICENSE` file — MIT or Apache-2.0; **pick before publishing**,
  since relicensing after contributors appear requires their agreement)
- `authors`, `readme = "README.md"`
- `[project.urls]` — Homepage / Repository / Issues
- `classifiers` — including `Operating System :: MacOS :: MacOS X`,
  `Programming Language :: Python :: 3.12`, `Environment :: Console`,
  `Development Status :: 3 - Alpha`
- `keywords`

Also in scope:

- **`README.md`** — install instructions using the §7.1 mechanism (`uv tool
  install` / `pipx install`), a `swab list` sample, and the config reference.
- **Platform guard** — per D5, fail with a clear message on non-macOS rather than
  half-installing.
- **CI** — run `pytest` on macOS against **Python 3.12 specifically**, not just the
  latest. Two shipped bugs (`__replace__`, `url2pathname(require_scheme=)`) were
  3.13+/3.14-only APIs that passed locally on 3.14 while violating the declared
  `>=3.12` floor. `.afk/check_pyver.py` catches the known cases by tokenizing, but a
  real 3.12 job is the only complete check.
- **Naming resolved (2026-08-06)** — see `DESIGN.md` §7.4: package `petridish`, CLI
  `swab`, hook `swab-hook`, data dir `~/.petridish/`, marker `# petridish`.

**Verify:** `python3 -m build` produces a wheel; `uv tool install --editable .`
yields working `swab` / `swab-hook` shims on `PATH`; `pytest` green on 3.12.

**Not in scope here:** publishing to PyPI, or authoring the Homebrew tap/formula.
Those follow once the name is settled.

---

## 7. Deferred (explicitly not in this plan)

Raycast extension · Textual TUI · FastAPI web UI · `ps`/`lsof` process sensing for
non-Claude CLIs (F8) · Copilot CLI (`~/.copilot/`) if it ever appears (F5) ·
multi-root VS Code workspaces · a `swab tui` resume-session action.
