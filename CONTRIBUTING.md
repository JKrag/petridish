# Contributing to petridish

Written for whoever picks this up next — human or AI agent. It says what to run, what must
not break, and which mistakes this codebase has already made so you don't repeat them.

## The one command

```sh
make check
```

Formatting, lints and the test suite — everything that needs nothing but a Rust
toolchain, so it stays fast enough to run constantly.

```sh
make check-all
```

Adds the three CI gates that need extra tooling: `cargo-deny`, a second toolchain for
the MSRV job, and node for the Raycast extension. **This is what CI runs in full**, so
run it before opening a PR. They are split because a gate that fails on a missing tool
trains people to ignore it.

```
make fmt        # cargo fmt --all
make fmt-check  # cargo fmt --all --check
make clippy     # cargo clippy --workspace --all-targets --all-features -- -D warnings
make test       # cargo test --locked --workspace -- --test-threads=1
make check      # fmt-check + clippy + test
make deny       # cargo-deny (licences + advisories)
make msrv       # build on the rust-version floor
make raycast    # the TypeScript extension
make check-all  # everything CI runs
```

The gates in `check` are **prerequisites, not one recipe line**. That is load-bearing: a
recipe like `cargo fmt --check; cargo test` returns only the *last* command's exit status,
so a formatting failure would report success. Don't "simplify" it.

One-time setup so `git blame` skips the bulk reformat commit:

```sh
git config blame.ignoreRevsFile .git-blame-ignore-revs
```

## Why `--test-threads=1`

Three tests in `swab/src/cli.rs` mutate `$HOME`, which is process-global, so in parallel
they race every test that reads it. Measured: without the flag, `cargo test -p swab --lib`
fails 3 runs out of 3.

This file and `CLAUDE.md` both used to blame the Python test suite for this. That was
wrong — the Python is gone and the requirement remains. Making those three tests stop
touching the environment is a real improvement and would let the flag go; everything
written since takes paths as parameters precisely to avoid adding a fourth.

## Non-negotiable invariants

Full list, and which crate each one binds: `CLAUDE.md`. Not restated here — the two drifted
out of sync once before (this file still described `subprocess.run(check=False)` git calls
long after `swab/src/git.rs` moved to in-process `gix`, with no "superseded" note), which
is exactly the failure mode a single source of truth avoids.

## Keep the dependency trees small

Not dogma, but not free either. `petridish-cli` in particular is the crate a user runs
first and the one that edits files it does not own, so it stays readable in one sitting:
`clap`, `serde_json`, `chrono`, `petridish-core`. PATH lookup, the scratch-directory test
helper and the `getuid` call are each a few lines of `std` rather than a dependency, on
purpose (ARCHITECTURE.md §8.3 D3).

`swab` and `petri` take what they need — `gix`, `ratatui`, `crossterm` — pinned in their
own manifests, with shared pins hoisted to `[workspace.dependencies]`.

`integrations/raycast/` is a separate MIT-licensed TypeScript extension that only ever
*reads* `~/.petridish/projects.json`. It may use whatever it likes and never links against
anything here.

## Testing

Real fixtures, not mocks. `git init` actual repos in tmpdirs with pinned author and date
env vars; write actual fixture transcript files; put real executables on a synthetic
`PATH`. Mocked subprocess output would have hidden most of the findings this project is
built on.

Where a seam is genuinely unavoidable — the `launchctl` calls — it is an injected trait
with a recording implementation that asserts the **exact argv sequence**, not a mock that
asserts nothing. The EALREADY bootout-and-retry path is three specific calls in a specific
order, and "it returned Ok" would not have caught the bug that motivated it.

**Tests must be hermetic.** Nothing here should read the real `~/.claude` or
`~/.petridish`. Prefer a parameter over an environment read: `petridish-cli` takes `home`,
`uid`, `claude_dir` and the `PATH` string as arguments so its tests never touch
process-global state, and so it can be tested on Linux in CI even though it only *runs* on
macOS.

### Write tests that can fail

The highest-value habit in this repo, and the one that has caught the most damage:

> **Before trusting a new test, run it against the *unfixed* code.**
> If it passes there, it proves nothing.

This is not hypothetical. Real examples from this codebase:

- A batch of tests for `run_scan`'s defaulting was written, passed, and was **reverted** —
  reintroducing the exact bug they targeted left all of them green. The coverage already
  existed elsewhere.
- A test used `next(line for line in lines if "--repo")`. That condition is a truthy
  constant, so it always matched the *first* line rather than the one under test. It
  passed while asserting nothing.
- A filter used `"--" in line`, which also matched the `---` separators. This one recurred
  in the Rust menubar tests — `l.starts_with("--")` matches the divider too.
- `swab/examples/golden_probe.rs` called itself the schema parity gate for the wire
  contract. It was an *example*: nothing ever invoked it, in CI or anywhere else. When it
  was finally rewritten as a real test it immediately failed, because the golden fixture
  had been stale for two schema additions. A gate nobody runs is not a gate.

A quick way to check a whole module: mutate an operator (`max`→`min`, flip a comparison,
add an off-by-one) and confirm the suite goes red.

Two mutations that survived while building the petri screens, both because the fixtures
never separated two fields that are usually set together:

- `is_dirty` was always derived from `uncommitted_files > 0`, so a renderer that checked
  only one of them passed. `has_agent` had the same problem with `last_event_at` and
  `session_id`. (The menubar deliberately requires *both* `is_dirty` and a non-zero count,
  and has a test for each half.)
- A sort key with an explicit group rank *plus* a negated second element: the sign silently
  did the partitioning, so removing the rank changed nothing and no test could tell. Two
  partitions concatenated is both clearer and actually testable.

If a mutation survives, the fix is a test, not a shrug.

### PTY tests

`petri/tests/pty_support/` drives the real binary through a pseudo-terminal. Two rules,
both learned the hard way:

- **Assert against a reconstructed screen grid, never the raw byte stream.** A
  partially-painted frame must surface as wrong content in a specific cell, not as a
  substring that coincidentally still matches. `petri/SPEC.md` §8 has the history.
- **A quiet-period timeout is a *trailing* bound, not a global one.** `settle(5s, 300ms)`
  originally returned after 300ms when nothing had arrived yet, conflating "finished
  painting" with "hasn't started". Across `screen_retry`'s five attempts that made the real
  first-output budget 1.5s rather than 5s, and the suite failed roughly half the time
  under load while passing every time on an idle machine.

## Things that look like bugs and aren't

- **Binary paths are absolutised but not canonicalised.** Resolving symlinks would bake
  Homebrew's version-stamped Cellar path into the launchd plist, and the next `brew
  upgrade` would leave the daemon pointing at a path that no longer exists.
- **`add_hook_entries` is idempotent per *event*, not per file.** A whole-file marker check
  reports "already installed" on machines set up before an event joined `HOOK_EVENTS`,
  stranding the new ones silently. See `settings.rs`.
- **`remove_marker_entries` prunes an event key it emptied, but not one that arrived
  empty.** The second is the user's own deliberate choice.
- **`href="file://..."` in the menubar output is double-quoted.** xbar splits `key=value`
  parameters on whitespace, so an unquoted path containing a space disables the plugin.
- **The xbar plugin is generated, not committed.** It has to embed an absolute binary path;
  see `integrations/xbar/README.md`.

## Layout

```
petridish-core/     shared wire schema, hook constants, presentation helpers
petridish-cli/      the `petridish` binary: install/uninstall/doctor/menubar
swab/               the scanner — the only writer of projects.json
petri/              the ratatui dashboard
fixtures/           shared JSON fixtures, consumed by tests in every crate
integrations/       xbar (docs) and raycast (a TypeScript extension, gated in CI)
```

The rule every frontend follows: **push logic into a pure function so it can be tested
without a display.** `petri` splits state from rendering (`petri/SPEC.md` §1);
`petridish-cli`'s `menubar.rs` is a pure `Radar -> String`. If you add a frontend, do the
same.

### The quota sensor reads someone else's file

Its lessons — reject `bool` before `int`, and drop timestamps more than 30 days out (the
classic seconds/milliseconds mixup) — are documented in `swab/src/sensors/quota.rs`'s own
module doc comment, next to the code they constrain. Read there, not here.
