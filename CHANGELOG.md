# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.0.0-beta.2] — unreleased

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

## [1.0.0-beta.1] — unreleased

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

[Unreleased]: https://github.com/JKrag/petridish/compare/v1.0.0-beta.2...HEAD
[1.0.0-beta.2]: https://github.com/JKrag/petridish/compare/v1.0.0-beta.1...v1.0.0-beta.2
[1.0.0-beta.1]: https://github.com/JKrag/petridish/releases/tag/v1.0.0-beta.1
