# petridish

Local monitoring daemon for macOS: crawls project roots, tracks git state, senses AI agent
activity, and aggregates into `~/.petridish/projects.json`.

**Two languages, split by role — not a partial migration, a deliberate split:**

- **`swab/`** (Rust) is the scanner: everything that *writes* `projects.json`. Binaries
  `swab` (CLI: `scan`/`list`/`path`/`doctor`/`config`) and `swab-hook` (fast hook path,
  appends to `events.ndjson`). This fully replaced the original Python scanner
  (`src/petridish/{cli,config,discovery,git,events,scan,hook}.py` +
  `sensors/{claude,copilot,quota}.py`, all deleted) after a from-scratch port proved
  field-equivalent (`swab/scripts/diff_check.sh`) and then measurably faster (gix
  in-process git access beats both a CLI-subprocess and a git2/libgit2 backend on real
  benchmarks — see `swab/src/git.rs`'s module doc comment for the full history).
- **`petridish-core/`** (Rust) is the schema + presentation layer shared between `swab`
  and the incoming Rust `petri` TUI (`petri/SPEC.md`, ADR-0002): the serde wire types
  (`Radar`/`Project`/`GitState`/... — moved out of `swab/src/schema.rs` to here) plus the
  pure derivation helpers both frontends need (agent label, dirty marker, worktree cell,
  bucket/activity strings). `swab` depends on it for these rather than owning them; it
  does not depend on `swab`, so it cannot reach the writer.
- **`src/petridish/`** (Python) is now read-side only: `schema.py` is the shared contract
  every frontend parses `projects.json` through (`Radar`/`Project` dataclasses,
  `read_json`), `tui.py`/`tui_state.py`/`screens.py` are `petripy`, the deprecated
  Python TUI kept installed as a fallback while the Rust `petri` (`petri/SPEC.md`) earns
  trust — see `CONTEXT.md`'s `petripy` entry, `menubar.py` is the menu-bar frontend,
  `installer.py` wires up the launchd job + Claude Code hook (invoking the Rust binaries
  by name via `shutil.which`). None of these write `projects.json` — they only ever read it.

**Read `ARCHITECTURE.md` before writing any code, in either language.** It's the
language-agnostic architecture/findings/schema doc — supersedes
`docs/archive/IMPLEMENTATION_PLAN.md` (the original all-Python build spec, kept only as a
historical record) for everything still true regardless of implementation.

## Stack & layout

A cargo workspace at the repo root (`Cargo.toml`, members `petridish-core` + `swab` today;
`petri` joins once its crate exists, `petri/SPEC.md` §2/§9 S4) sits alongside the Python
package:

- **`petridish-core/`**: Rust, `serde` + `chrono`. Schema types (`src/schema.rs`, moved from
  `swab/src/schema.rs`) plus the shared `present` derivation helpers. No `swab` or `petri`
  dependency — the compiler enforces that this crate can't reach the state-file writer.
- **`swab/`**: Rust, `gix` (pure-Rust git, no libgit2/C dependency) + `clap` + `serde` +
  `chrono` + `regex` + `toml`, plus `petridish-core` for the schema/`present` types. Source
  in `swab/src/`, sensors in `swab/src/sensors/`, tests inline (`#[cfg(test)] mod tests`) in
  each module. Verified via `cargo test --workspace` plus `swab/scripts/diff_check.sh` (a
  differential oracle — no longer has a Python scanner to diff against, so treat its
  fixture-based golden comparisons and real regression tests as the correctness bar
  instead).
- **`src/petridish/`**: Python 3.12+, stdlib only, for everything that remains here.
  `pytest` is the sole dev dependency. Do not add runtime dependencies to this side — the
  zero-deps constraint is what keeps the TUI/menubar/installer trivially verifiable with no
  env setup. (This constraint never applied to the Rust crates — their dependencies are
  fine, pinned in each crate's own `Cargo.toml`.)
- Tests in `tests/` (Python, pytest) and `{petridish-core,swab}/src/**/*.rs` (Rust, inline
  `#[test]`), run together via `cargo test --workspace` from the repo root.

## Non-negotiable invariants

These encode findings verified on the real machine. Violating one produces code that passes
tests and is still wrong. Invariants 1-5 apply to `swab/`, the only thing still writing
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
   `swab/src/git.rs` now calls `gix` in-process (no subprocess, no timeout construct) for
   everything except nothing — there's no CLI fallback left at all. The invariant this
   protected still holds in spirit: a git failure degrades to `GitState { is_repo: false,
   .. }`, never a panic or an exception, enforced by `gix::open`'s `Result` and `?`-free
   fallback matching throughout `git.rs`.

## Testing

**Rust (`petridish-core/`, `swab/`)**: real fixtures, not mocks — `git init` actual repos in
tmpdirs with pinned author/date env vars, real fixture transcript files, cross-verified
against the real `git` CLI's own porcelain output where behavior is subtle (see `git.rs`'s
status-parity regression tests). `cargo test --workspace -- --test-threads=1` from the repo
root must exit 0 (parallel test threads currently share `HOME` env-var mutation across some
Python-side fixture tests only — not a Rust issue, but run single-threaded out of habit if
in doubt).

**Python (`src/petridish/`)**: same real-fixtures philosophy for whatever exercises
`schema.py`/the TUI/installer. `pytest tests/ -q` from the repo root must exit 0.

## Engineering integrity

Correctness over green checks. Do not weaken, skip, or delete a test to make it pass — if a
check fails, fix the code. If something cannot be built as specified, **stop and escalate
with the reason**; do not narrow the spec, stub a sensor, or silently substitute a simpler
approach. Any deliberate shortcut needs a comment at the site and a note in the summary.

<!-- rtk-instructions v2 -->
# RTK (Rust Token Killer) - Token-Optimized Commands

## Golden Rule

**Always prefix commands with `rtk`**. If RTK has a dedicated filter, it uses it. If not, it passes through unchanged. This means RTK is always safe to use.

**Important**: Even in command chains with `&&`, use `rtk`:
```bash
# ❌ Wrong
git add . && git commit -m "msg" && git push

# ✅ Correct
rtk git add . && rtk git commit -m "msg" && rtk git push
```

## RTK Commands by Workflow

### Build & Compile (80-90% savings)
```bash
rtk cargo build         # Cargo build output
rtk cargo check         # Cargo check output
rtk cargo clippy        # Clippy warnings grouped by file (80%)
rtk tsc                 # TypeScript errors grouped by file/code (83%)
rtk lint                # ESLint/Biome violations grouped (84%)
rtk prettier --check    # Files needing format only (70%)
rtk next build          # Next.js build with route metrics (87%)
```

### Test (60-99% savings)
```bash
rtk cargo test          # Cargo test failures only (90%)
rtk go test             # Go test failures only (90%)
rtk jest                # Jest failures only (99.5%)
rtk vitest              # Vitest failures only (99.5%)
rtk playwright test     # Playwright failures only (94%)
rtk pytest              # Python test failures only (90%)
rtk rake test           # Ruby test failures only (90%)
rtk rspec               # RSpec test failures only (60%)
rtk test <cmd>          # Generic test wrapper - failures only
```

### Git (59-80% savings)
```bash
rtk git status          # Compact status
rtk git log             # Compact log (works with all git flags)
rtk git diff            # Compact diff (80%)
rtk git show            # Compact show (80%)
rtk git add             # Ultra-compact confirmations (59%)
rtk git commit          # Ultra-compact confirmations (59%)
rtk git push            # Ultra-compact confirmations
rtk git pull            # Ultra-compact confirmations
rtk git branch          # Compact branch list
rtk git fetch           # Compact fetch
rtk git stash           # Compact stash
rtk git worktree        # Compact worktree
```

Note: Git passthrough works for ALL subcommands, even those not explicitly listed.

### GitHub (26-87% savings)
```bash
rtk gh pr view <num>    # Compact PR view (87%)
rtk gh pr checks        # Compact PR checks (79%)
rtk gh run list         # Compact workflow runs (82%)
rtk gh issue list       # Compact issue list (80%)
rtk gh api              # Compact API responses (26%)
```

### JavaScript/TypeScript Tooling (70-90% savings)
```bash
rtk pnpm list           # Compact dependency tree (70%)
rtk pnpm outdated       # Compact outdated packages (80%)
rtk pnpm install        # Compact install output (90%)
rtk npm run <script>    # Compact npm script output
rtk npx <cmd>           # Compact npx command output
rtk prisma              # Prisma without ASCII art (88%)
rtk uv run <cmd>        # Compact uv project command output
```

### Files & Search (60-75% savings)
```bash
rtk ls <path>           # Tree format, compact (65%)
rtk read <file>         # Code reading with filtering (60%)
rtk grep <pattern>      # Search grouped by file (75%). Format flags (-c, -l, -L, -o, -Z) run raw.
rtk find <pattern>      # Find grouped by directory (70%)
```

### Analysis & Debug (70-90% savings)
```bash
rtk err <cmd>           # Filter errors only from any command
rtk log <file>          # Deduplicated logs with counts
rtk json <file>         # JSON structure without values
rtk deps                # Dependency overview
rtk env                 # Environment variables compact
rtk summary <cmd>       # Smart summary of command output
rtk diff                # Ultra-compact diffs
```

### Infrastructure (85% savings)
```bash
rtk docker ps           # Compact container list
rtk docker images       # Compact image list
rtk docker logs <c>     # Deduplicated logs
rtk kubectl get         # Compact resource list
rtk kubectl logs        # Deduplicated pod logs
```

### Network (65-70% savings)
```bash
rtk curl <url>          # Compact HTTP responses (70%)
rtk wget <url>          # Compact download output (65%)
```

### Meta Commands
```bash
rtk gain                # View token savings statistics
rtk gain --history      # View command history with savings
rtk discover            # Analyze Claude Code sessions for missed RTK usage
rtk proxy <cmd>         # Run command without filtering (for debugging)
rtk init                # Add RTK instructions to CLAUDE.md
rtk init --global       # Add RTK to ~/.claude/CLAUDE.md
```

## Token Savings Overview

| Category | Commands | Typical Savings |
|----------|----------|-----------------|
| Tests | vitest, playwright, cargo test | 90-99% |
| Build | next, tsc, lint, prettier | 70-87% |
| Git | status, log, diff, add, commit | 59-80% |
| GitHub | gh pr, gh run, gh issue | 26-87% |
| Package Managers | pnpm, npm, npx | 70-90% |
| Files | ls, read, grep, find | 60-75% |
| Infrastructure | docker, kubectl | 85% |
| Network | curl, wget | 65-70% |

Overall average: **60-90% token reduction** on common development operations.
<!-- /rtk-instructions -->