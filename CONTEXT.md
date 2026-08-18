# petridish — Glossary

Terms resolved during design discussions, kept here so they don't drift.
Implementation details belong in `ARCHITECTURE.md` / `DESIGN.md`, not here.

## `swab` vs `petri`

Two separate console scripts, both installed by the `petridish` package, with
different jobs:

- **`swab`** — the scanner/CLI (Rust, `swab/`). Runs the crawl, writes `projects.json`
  (`swab scan`), and exposes non-interactive inspection commands (`list`,
  `path`, `doctor`, `config`). Named for "swabbing the environment for
  life" — the sensing side of the metaphor.
- **`petri`** — the interactive frontend. Launches the terminal dashboard that renders
  `projects.json` for a human to look at (currently Python/`curses`; slated for a
  Go/Bubbletea or Rust/ratatui rewrite — see `ARCHITECTURE.md` §6 for the behavior spec any
  reimplementation needs to satisfy). Read-only consumer of the state file; never writes
  it. Named for the petri dish itself — the thing you look *into*.

`petri` is TUI-only for now. `swab`'s subcommands are not migrating under
it; that merge is explicitly out of scope until/unless revisited.

## Status buckets

`active` / `in_flight` / `stale` / `cold` — see `swab/src/schema.rs`'s
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
