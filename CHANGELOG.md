# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.0.0-beta.5] — 2026-09-11

### Changed

- `swab`: `git::scan` no longer silently discards `gix::open` failures. A
  plain "this directory has no `.git`" (`gix::open::Error::NotARepository`)
  still degrades to `is_repo: false` with no output, since most scanned paths
  legitimately aren't repos — but every other failure (a malformed gitdir,
  permission/ownership issues, bad config) is now written to stderr with the
  resolved path, which the launchd plist already routes into `daemon.log`.
  This is diagnostic instrumentation for #63 (a project intermittently
  flipping from `git: main` to "Not a git repo" between scans), not a fix —
  the underlying flap hasn't been reproduced yet.

## [1.0.0-beta.4] — 2026-09-11

### Added

- `swab`: `exclude_paths` config option to drop a whole subtree from the fleet
  by path prefix, e.g. agent scratch directories under `/private/tmp/...` that
  `ignore_dirs` could never reach (it only matches on basename, and only
  during the crawl). `swab doctor` now also flags an exclusion that
  accidentally swallows a configured root or `extra_paths` entry whole (#46).
- `petri`: `--help` / `-h` (#42).
- `petridish_core`: a single `SCHEMA_VERSION` constant, replacing a literal
  `1` copy-pasted across 20 sites in four crates. No behavior change yet —
  nothing compares against it — but it closes the gap that let a real
  `swab`/`petri` version mismatch ship in beta.3 (#54).

### Fixed

- `petri`: the alternate screen is now guarded by a `Drop` impl, so an error
  partway through startup can no longer leave the shell stuck rendering into
  the TUI's alt buffer (#45).
- `swab`: `exclude_paths` is now resolved once per scan instead of once per
  directory visited, removing an O(directories × exclusions) cost on large
  roots.

### Changed

- `petri`'s PTY test harness now parses terminal output with `vt100` instead
  of a hand-rolled ANSI parser, closing gaps (scroll regions, line wrapping,
  relative cursor movement) the old parser silently dropped (#47). Its test
  suite was also audited end-to-end, dropping two tests that duplicated an
  existing non-PTY snapshot test (#48).

## [1.0.0-beta.3] — 2026-09-10

### Added

- `petri` focus panel: one responsive renderer behind #30/#31/#32, mounted two
  ways — a Dashboard popup and `petri --mini [NAME]`, a whole-screen
  single-project view. `--mini NAME` resolves to the most recently active match;
  with no operand the cwd is resolved against `projects.json`'s own roots,
  deepest first, rather than by a second copy of the scanner's `resolve_root`.
- `petri` Dashboard: a `lush` density tier above `roomy` (SPACE-3, #33). RUNNING
  cards carry `last` and `repo`, under a bounded per-card claim on surplus rows
  so the SPACE-1 feed still gets the remainder.
- `petri`: Claude quota in the header on both screens — `5h 16% · 7d 1%`, or the
  compressed `16%/1%` — display-only from the `Radar.quota` the scanner already
  writes. A half that degraded to `None` is omitted rather than printed as `0%`.
  The header's right-hand group gained the elision ladder it never had (#29).
- `petri` Browser: a git segment in the list row. `!N` modified, `?N` untracked,
  zero counts omitted, and a non-repo gets a positive mark rather than an empty
  cell. Nerd Font glyphs are opt-in (#38).
- `swab`: `git.untracked_files`, counted separately from modified ones and
  guarded so ignored entries do not inflate it. `uncommitted_files` keeps its
  original meaning as the total (#38).
- `--version` on `swab`, `swab-hook` and `petri`, none of which had it (#36).
- `petridish doctor`: a version-agreement check across the four binaries, an
  `N/N checks passed` summary line, and a `--json` output mode carrying the same
  checks and the same exit code.
- `make flake-hunt` (`petri/scripts/flake-hunt.sh`): runs each PTY test binary N
  times at concurrency and reports a per-test failure rate. Deliberately not part
  of `make check`.

### Changed

- `petri` Browser list rows are column-aligned, and the chrome collapses last as
  the pane narrows — gaps first, then the row leader from the outside in (#38).
- The `g` action is git-aware: on a non-repo it shows a notice instead of
  launching a git tool that would exit immediately (#38).
- `git log` hand-offs pin `core.pager=less -+F -R`, so a history short enough to
  fit one screen stays open instead of flashing past.

### Fixed

- `swab` no longer keeps project roots that are no longer on disk. The entry was
  not stale state — signal roots (a `cwd` read from a transcript or an event)
  re-supplied the dead path every tick, so a rescan looked like a no-op (#41).
- `petri` Browser: on a terminal too narrow for a side-by-side detail pane but
  tall enough, the pane reflows below the list instead of disappearing, and its
  height grows with the terminal. When neither placement fits it is still
  reachable as a `Space` popup (#35).
- `petri` Browser rows could wrap below seven columns, which misaligns every row
  beneath them, since the list clips nothing and scrolling counts lines.
- The Browser's shrink step could *grow* a column, inverting the ladder it
  implements.
- Ages no longer truncate into a different claim (`20d ago` → `20d` → `2`); the
  age column is all-or-nothing and blanks rather than clips.
- A *directory* named `Nerd Fonts/` passed the font probe, enabling PUA glyphs
  for someone with no font installed. Directories are traversed, never answers.
- The tool cache is invalidated when a re-pick changes the answer.
- `yank_selected_path`'s budget is 10 attempts, not 30: issue #49 ("`y` blocks
  for ~2s") was measured on a machine carrying leaked busy-loop processes and is
  closed as mistaken. On an idle machine `pbcopy` takes 10-20ms.

### Testing

- Every flaky PTY wait is now a condition rather than a duration. Measured at
  eight-way concurrency before the fix, `s8_pty_repick` failed 17 runs in 24,
  `s8_pty_actions` 15 in 24, `s8_pty_filter` 10 in 24. Three rules came out of
  it, each learned by getting it wrong first: the predicate must be false for the
  pre-keystroke frame; an absence needs `settle_until_gone`; a keystroke that
  hands over the terminal is not observable on the grid at all. See CLAUDE.md.
- `"petri"` is not a startup marker — `prefs::load`'s warning is printed before
  `enable_raw_mode`, so the stream is non-empty while a keystroke is still being
  swallowed by the line discipline. Tests wait for a painted header badge or for
  the alternate-screen entry itself.
- `swab`'s quota sensor takes an injected clock, so its tests stop expiring.
- No test mutates `$HOME` to fake it; the detail-popup PTY test isolates its own.

### Known issues

- `y` cannot copy on Linux: `pbcopy` is macOS-only (#50).

## [1.0.0-beta.2] — 2026-09-07

### Added

- `petri` Browser: `f` reveals the selected project in Finder, `s` triggers an
  immediate rescan, `y` yanks the project path to the clipboard, and `?` opens a
  help popup listing all bound action keys (ACT-2, #27).
- `petri` Browser: named-browser, `ranger`/`nnn`, and `gitup`/`gitcomet` entries
  as additional `browse`/`reveal`/`gitlog` tool candidates.

### Fixed

- App-bundle probes now also check `/System/Applications`, not just
  `/Applications`.
- Tool picker rows and the "no tool" message show a candidate's `id`, not its
  `program`.
- `yank_selected_path` reaps the `pbcopy` child on write failure instead of
  leaking it.
- `Candidate` now has an identity distinct from its `program`.

## [1.0.0-beta.1] — 2026-09-05

First public release. Everything before this lived only in git history.

### Added

- `petridish`, a new binary owning `install`, `uninstall`, `doctor` and `menubar`.
  It is the first command a new user runs.
- `petridish doctor`, checking the *install* surface: binaries resolve to absolute
  paths, the launchd plist still points at a binary that exists, every hook event
  is registered, and the menu-bar plugin is present and executable. Distinct from
  `swab doctor`, which checks scanner health.
- Distribution via a Homebrew tap: `brew install jkrag/tap/petridish`.
- Quality gates in CI that did not exist before — `cargo fmt --check`, `cargo
  clippy -D warnings`, an MSRV job, `cargo-deny` for licences and advisories, and
  tests on Linux as well as macOS.
- The Raycast extension is now gated in CI (tsc, tests, eslint, prettier). It had
  never been checked by anything.
- Golden-fixture and timestamp-format tests in `petridish-core`, replacing
  coverage that lived only in the deleted Python suite.

### Changed

- **The project is Rust-only.** The Python read-side — `petripy`, `schema.py`,
  `menubar.py`, `installer.py` — is gone; see ADR-0004.
- One version across all crates, from `[workspace.package]`.
- Shared test fixtures moved from the Python test tree to `/fixtures`.
- `raycast/` moved to `integrations/raycast/`; `integrations/xbar/` documents the
  menu-bar plugin.

### Fixed

- `events.ndjson`'s key order is pinned by a type rather than by a `BTreeMap`
  accident, so it cannot be changed as a side effect of a Cargo feature.
- The golden fixture was two schema fields out of date (`git.daily_commits`,
  `project.agent_activity`) and nothing caught it — its only Rust gate was an
  example nothing ran.
- A PTY test harness bug that made the suite fail roughly half the time under load:
  the quiet-period timeout was applied to a still-empty buffer, cutting the
  first-output budget from 5s to 1.5s.
- `.gitignore`'s Node rules were path-anchored and silently stopped matching when
  the Raycast extension moved.
- `cargo test` no longer requires `--test-threads=1`. Three tests in
  `swab/src/cli.rs` used to mutate `$HOME`, and `swab-hook`'s tests mutated
  `$PETRIDISH_EVENTS_PATH`, both process-global; `cmd_doctor`, `config::load_config`
  and `handle_hook_input`/`run_hook` now take `home`/`events_path` as parameters
  instead. See issue #20.

### Known issues

- The Raycast extension cannot be published to the Raycast Store: the Store
  requires MIT and this project is GPL-3.0-or-later. See
  `integrations/raycast/README.md`.

[Unreleased]: https://github.com/JKrag/petridish/compare/v1.0.0-beta.3...HEAD
[1.0.0-beta.3]: https://github.com/JKrag/petridish/compare/v1.0.0-beta.2...v1.0.0-beta.3
[1.0.0-beta.2]: https://github.com/JKrag/petridish/compare/v1.0.0-beta.1...v1.0.0-beta.2
[1.0.0-beta.1]: https://github.com/JKrag/petridish/releases/tag/v1.0.0-beta.1
