# petridish

Local monitoring daemon for macOS: crawls project roots, tracks git state, senses AI agent
activity, and aggregates into `~/.petridish/projects.json`.

**Two languages, split by role — not a partial migration, a deliberate split:**

- **`swab-rs/`** (Rust) is the scanner: everything that *writes* `projects.json`. Binaries
  `swab` (CLI: `scan`/`list`/`path`/`doctor`/`config`) and `swab-hook` (fast hook path,
  appends to `events.ndjson`). This fully replaced the original Python scanner
  (`src/petridish/{cli,config,discovery,git,events,scan,hook}.py` +
  `sensors/{claude,copilot,quota}.py`, all deleted) after a from-scratch port proved
  field-equivalent (`swab-rs/scripts/diff_check.sh`) and then measurably faster (gix
  in-process git access beats both a CLI-subprocess and a git2/libgit2 backend on real
  benchmarks — see `swab-rs/src/git.rs`'s module doc comment for the full history).
- **`src/petridish/`** (Python) is now read-side only: `schema.py` is the shared contract
  every frontend parses `projects.json` through (`Radar`/`Project` dataclasses,
  `read_json`), `tui.py`/`tui_state.py`/`screens.py` are the `petri` dashboard,
  `menubar.py` is the menu-bar frontend, `installer.py` wires up the launchd job + Claude
  Code hook (invoking the Rust binaries by name via `shutil.which`). None of these write
  `projects.json` — they only ever read it.

**Read `ARCHITECTURE.md` before writing any code, in either language.** It's the
language-agnostic architecture/findings/schema doc — supersedes
`docs/archive/IMPLEMENTATION_PLAN.md` (the original all-Python build spec, kept only as a
historical record) for everything still true regardless of implementation.

## Stack & layout

- **`swab-rs/`**: Rust, `gix` (pure-Rust git, no libgit2/C dependency) + `clap` + `serde` +
  `chrono` + `regex` + `toml`. Source in `swab-rs/src/`, sensors in
  `swab-rs/src/sensors/`, tests inline (`#[cfg(test)] mod tests`) in each module. Verified
  via `cargo test` plus `swab-rs/scripts/diff_check.sh` (a differential oracle — no longer
  has a Python scanner to diff against, so treat its fixture-based golden comparisons and
  real regression tests as the correctness bar instead).
- **`src/petridish/`**: Python 3.12+, stdlib only, for everything that remains here.
  `pytest` is the sole dev dependency. Do not add runtime dependencies to this side — the
  zero-deps constraint is what keeps the TUI/menubar/installer trivially verifiable with no
  env setup. (This constraint never applied to `swab-rs/` — Rust dependencies there are
  fine, pinned in `swab-rs/Cargo.toml`.)
- Tests in `tests/` (Python, pytest) and `swab-rs/src/**/*.rs` (Rust, inline `#[test]`).

## Non-negotiable invariants

These encode findings verified on the real machine. Violating one produces code that passes
tests and is still wrong. Invariants 1-5 apply to `swab-rs/`, the only thing still writing
`projects.json`; invariant 6 has been superseded (see note).

1. **Single writer.** Only `swab scan` writes `projects.json`, via temp-file + atomic
   rename. `swab-hook` appends one line to `events.ndjson` and nothing else. Never make the
   hook touch `projects.json` — three other hook consumers already share these events.
2. **Never parse a path out of a `~/.claude/projects/` dirname.** The slug encodes `/` and
   `-` identically and is not reversible. Read `cwd` from the JSONL contents.
3. **`cwd` varies within one transcript.** Take it from the *last* parseable line, then run
   it through `resolve_root()` so monorepo subdirs collapse to one project.
4. **Truncated trailing JSONL lines are normal**, not errors — live sessions are being
   appended to as you read. Skip and fall back to the previous line.
5. **Sensors degrade, never abort.** A failing sensor yields `null` fields; the tick still
   writes a complete file.
6. ~~**`git` calls use `subprocess.run` with `check=False` and a 5s timeout.**~~ Superseded:
   `swab-rs/src/git.rs` now calls `gix` in-process (no subprocess, no timeout construct) for
   everything except nothing — there's no CLI fallback left at all. The invariant this
   protected still holds in spirit: a git failure degrades to `GitState { is_repo: false,
   .. }`, never a panic or an exception, enforced by `gix::open`'s `Result` and `?`-free
   fallback matching throughout `git.rs`.

## Testing

**Rust (`swab-rs/`)**: real fixtures, not mocks — `git init` actual repos in tmpdirs with
pinned author/date env vars, real fixture transcript files, cross-verified against the real
`git` CLI's own porcelain output where behavior is subtle (see `git.rs`'s status-parity
regression tests). `cargo test -- --test-threads=1` from `swab-rs/` must exit 0 (parallel
test threads currently share `HOME` env-var mutation across some Python-side fixture tests
only — not a Rust issue, but run single-threaded out of habit if in doubt).

**Python (`src/petridish/`)**: same real-fixtures philosophy for whatever exercises
`schema.py`/the TUI/installer. `pytest tests/ -q` from the repo root must exit 0.

## Engineering integrity

Correctness over green checks. Do not weaken, skip, or delete a test to make it pass — if a
check fails, fix the code. If something cannot be built as specified, **stop and escalate
with the reason**; do not narrow the spec, stub a sensor, or silently substitute a simpler
approach. Any deliberate shortcut needs a comment at the site and a note in the summary.
