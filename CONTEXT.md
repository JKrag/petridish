# petridish — Glossary

Terms resolved during design discussions, kept here so they don't drift.
Implementation details belong in `ARCHITECTURE.md` / `petri/SPEC.md`, not here
(`docs/archive/DESIGN.md` is the original pre-implementation pitch doc, archived).

## `swab` vs `petri`

Two separate console scripts, both installed by the `petridish` package, with
different jobs:

- **`swab`** — the scanner/CLI (Rust, `swab/`). Runs the crawl, writes `projects.json`
  (`swab scan`), and exposes non-interactive inspection commands (`list`,
  `path`, `doctor`, `config`). Named for "swabbing the environment for
  life" — the sensing side of the metaphor.
- **`petri`** — the interactive frontend. Launches the terminal dashboard that renders
  `projects.json` for a human to look at. Read-only consumer of the state file; never
  writes it. Named for the petri dish itself — the thing you look *into*. As of the
  Rust edition, `petri` means the Rust/ratatui build; the original Python/`curses`
  build is [[petripy]] (below) and is deprecated, kept only until the Rust one
  replaces it in daily use.
  _Avoid_: calling the Rust build a "port" — it is a reimplementation that keeps the
  behavior and discards the curses-shaped internals (see "Screens" below).

`petri` is TUI-only for now. `swab`'s subcommands are not migrating under
it; that merge is explicitly out of scope until/unless revisited.

## `petripy`

The original Python/`curses` `petri` (`src/petridish/{tui,tui_state,screens}.py`),
renamed to `petripy` when the Rust edition took the `petri` name. Deprecated on
arrival of the Rust build but deliberately kept installed and working — it is the
reference for what "feature parity at the abstract level" means, and the fallback
while the Rust one earns trust. Not a supported long-term surface; no new features
land here.
_Avoid_: "the old petri", "legacy petri" — it has a name.

## Screens: Dashboard and Browser

`petri` is two screens, tab-switched, with different jobs. This split is kept in the
Rust edition; more screens may be added later.

- **Dashboard** — the *ambient monitor*. Answers "does anything need me?" across a
  fleet of unattended agent runs: RUNNING/RECENT with the quietest run first, then
  IN FLIGHT, STALE, COLD in decreasing prominence. You leave it open in a pane and
  glance at it.
- **Browser** — the *tool you drive*. Dense list plus a detail pane, search, and
  selection. Density beats prominence; every project must be reachable.

**The Dashboard is navigable, not inert.** The Python build made it strictly
input-free so it could be a pure function; in real use that was the wrong trade —
the hands want to move. In the Rust edition it is a screen you can move around in.
Selection is therefore a concept shared by both screens, not a Browser-only one.
_Avoid_: "read-only screen", "static view" for the Dashboard — read-only refers to
the [[state file]], not to whether you can put a cursor on something.

## Status buckets

`active` / `in_flight` / `stale` / `cold` — see `petridish-core/src/schema.rs`'s
`StatusBucket` (or Python `schema.py`'s `STATUS_BUCKETS` on the read side) and
`ARCHITECTURE.md` §4 for the time thresholds. Every frontend (CLI table, `petri`'s grouped
sections) organizes primarily by this axis.

## Foreign project

A project discovered under a configured root that isn't attributable to the
user (see the authorship filter, `ARCHITECTURE.md` §2) — `is_foreign`
on `Project`. Hidden by default in both `swab list` and `petri`; no `--all`
equivalent toggle in `petri` v1.

## Worktree project

A `Project` whose resolved path contains a `.worktrees/<name>` segment —
i.e. it was created under the `.worktrees/` convention shared by the
`worktree-provision`/`feature-branch`/`using-git-worktrees` skills, not an
ad-hoc `git worktree add` elsewhere on disk. Tracked as its own independent
`Project` (own git state, own agent signals, own `status_bucket`) — never
collapsed into its parent, because `resolve_root` deliberately stops at the
first `.git` it finds (a worktree has its own) rather than walking up.
`parent_path` on `Project` points back at the containing project's resolved
path; `null` for everything else. Its own activity (commits, uncommitted
files, agent sessions run with cwd inside it) never marks the parent active
— they share only the underlying object database (`commondir`), not working
directory, index, or branch. See ADR-0001 for why detection is a path
convention rather than git-native, and why the "parent counts as active if
a worktree child is active" rule lives only in `petri`'s display logic, not
in the `status_bucket` written to `projects.json`.
_Avoid_: "nested project", "sub-project" — a worktree project is a peer
`Project` entry, not contained inside its parent's own entry.

## State file vs preferences file

Two distinct things, kept distinct because the "frontends never write" shorthand is
only true of the first.

- **State file** — the aggregated picture of every project, produced by `swab scan`
  and by nothing else ever. Every frontend, `petri` included, is a read-only
  consumer of it.
- **Preferences file** — a frontend's own remembered UI choices. Owned by the
  frontend that wrote it and read by nothing else.

So "`petri` never writes" is imprecise. The accurate statement: `petri` never writes
the *state file*, and owns exactly one preferences file of its own. Paths and
mechanics: `petri/SPEC.md`.
_Avoid_: calling either one "the config" — `config.toml` is a third thing, the
scanner's own hand-edited settings.
