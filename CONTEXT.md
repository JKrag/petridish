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
