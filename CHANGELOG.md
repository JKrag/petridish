# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.0.0-beta.8] — 2026-09-14

### Added

- **Prebuilt Linux binaries.** `brew install jkrag/tap/petridish` now works
  on Linux (x86_64) via Linuxbrew, no Rust toolchain or build-from-source
  needed. Other architectures (e.g. arm64) still use `cargo install`.

## [1.0.0-beta.7] — 2026-09-14

### Added

- **Linux is now supported.** `petridish install`/`uninstall` work via
  `systemd --user` — same behavior as macOS (#75).
- **`petri --mini`**: shortcut keys that don't resolve now show a quick
  notice instead of doing nothing silently (#65).

### Fixed

- Linux install now respects `$XDG_CONFIG_HOME` instead of assuming
  `~/.config`.
- Linux uninstall now fully stops the scan service and reloads systemd —
  nothing lingers after uninstall.
- Fixed a unit-file bug that could break install on paths containing `%`.

## [1.0.0-beta.6] — 2026-09-12

### Added

- **Schema-drift warning**: the Dashboard, Browser, `--mini`, and `menubar`
  now warn when `projects.json` was written by a newer, incompatible
  `swab` (#54).
- Tool-shortcut keys (`e`/`o`/`g`/etc.) now work from the Dashboard's focus
  popup and from `petri --mini`, not just the Browser (#64).
- Linux: a VS Code Copilot activity sensor, matching the existing macOS
  one (#23).
- Linux: an `xdg-open` fallback for the browse action (#24).

### Fixed

- `doctor`/`menubar` now report "not applicable" on non-macOS instead of
  failing outright (#25).
- Fixed the git-log action's fallback path leaking into the user's shell
  (#62).

### Changed

- Docs: a new "Linux" section in the README (#26).

## [1.0.0-beta.5] — 2026-09-11

### Changed

- `swab` now logs unexpected git-scan errors instead of silently
  discarding them — diagnostic groundwork for tracking down #63.

## [1.0.0-beta.4] — 2026-09-11

### Added

- `swab`: `exclude_paths` config option to drop a whole subtree from the
  fleet by path (#46).
- `petri`: `--help` / `-h` (#42).

### Fixed

- `petri`: the alternate screen no longer gets stuck rendering after a
  startup error (#45).
- `swab`: fixed a performance issue scanning large roots with
  `exclude_paths` configured.

## [1.0.0-beta.3] — 2026-09-10

### Added

- New focus panel: a Dashboard popup and `petri --mini [NAME]`, a
  whole-screen single-project view (#29-#33).
- Dashboard: a `lush` density tier for wider terminals (#33).
- Claude quota shown in the header — `5h 16% · 7d 1%` (#29).
- Browser: a git status column, `!N` modified / `?N` untracked (#38).
- `--version` on `swab`, `swab-hook`, and `petri` (#36).
- `petridish doctor`: a version-agreement check, a summary line, and a
  `--json` output mode.

### Changed

- Browser list rows are column-aligned and collapse gracefully as the
  pane narrows (#38).
- `g` shows a notice on a non-repo instead of launching a tool that would
  just exit (#38).
- `git log` output now stays open when it's short instead of flashing
  past.

### Fixed

- `swab` no longer keeps project roots that are no longer on disk (#41).
- Browser: the detail pane now reflows below the list on narrow terminals
  instead of disappearing (#35).
- Fixed several Browser layout bugs: row wrapping below seven columns, a
  shrink step that could grow a column, and age-column values truncating
  into a different claim.
- Fixed a false Nerd Font detection caused by a directory literally named
  `Nerd Fonts/`.
- Closed #49 ("`y` blocks for ~2s") as a measurement artifact — on an
  idle machine `pbcopy` takes 10-20ms.

### Known issues

- `y` (copy path) doesn't work on Linux yet — `pbcopy` is macOS-only
  (#50).

## [1.0.0-beta.2] — 2026-09-07

### Added

- Browser: `f` reveals the selected project in Finder, `s` triggers an
  immediate rescan, `y` copies the project path, and `?` opens a help
  popup (#27).
- Browser: added `ranger`/`nnn` and `gitup`/`gitcomet` as tool
  candidates.

### Fixed

- App-bundle detection now also checks `/System/Applications`.
- Tool picker rows now show a candidate's name instead of its raw
  program path.
- Fixed a child-process leak when copying the path failed.

## [1.0.0-beta.1] — 2026-09-05

First public release. Everything before this lived only in git history.

### Added

- `petridish`, a new binary owning `install`, `uninstall`, `doctor`, and
  `menubar` — the first command a new user runs.
- `petridish doctor`: checks the install itself (binaries, the launchd
  plist, hook registration, the menu-bar plugin).
- Distribution via a Homebrew tap: `brew install jkrag/tap/petridish`.
- CI quality gates: formatting, lint, an MSRV check, licence/advisory
  scanning, and tests on both Linux and macOS.

### Changed

- **The project is Rust-only.** The old Python read-side is gone; see
  ADR-0004.
- One version number across all crates.

### Fixed

- Fixed a golden test fixture that was two schema fields out of date.
- Fixed a flaky test suite (roughly half the runs failed under load).

### Known issues

- The Raycast extension can't be published to the Raycast Store — its
  licence requirement doesn't match this project's GPL-3.0-or-later. See
  `integrations/raycast/README.md`.

[Unreleased]: https://github.com/JKrag/petridish/compare/v1.0.0-beta.3...HEAD
[1.0.0-beta.3]: https://github.com/JKrag/petridish/compare/v1.0.0-beta.2...v1.0.0-beta.3
[1.0.0-beta.2]: https://github.com/JKrag/petridish/compare/v1.0.0-beta.1...v1.0.0-beta.2
[1.0.0-beta.1]: https://github.com/JKrag/petridish/releases/tag/v1.0.0-beta.1
