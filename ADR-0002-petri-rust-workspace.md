# `petri` in Rust: cargo workspace with a shared `petridish-core` crate

**Status:** accepted

The Python/curses `petri` is being reimplemented in Rust/ratatui. That reopens a
question the single-binary `swab` never had to answer: where does the new code live
relative to the scanner, and what do the two share?

**A workspace with `petri` as its own crate, not a third `[[bin]]` in `swab`.**
Adding `petri` as another binary in the existing crate is the zero-refactor option
and keeps `cargo install --path swab` installing everything. Rejected because
`swab-hook` is the declared latency path (it runs on every Claude Code hook
invocation and does one append to `events.ndjson`), and a crate whose dependency
tree contains ratatui and crossterm is the wrong home for it. Cargo does
dead-code-eliminate per binary, so this is not primarily about the shipped hook's
size — it is that the dependency tree, build times and `cargo test` blast radius
would all be shared, and the boundary between "writes the state file" and "reads the
state file" would stop being enforced by anything but discipline. A separate crate
makes the compiler enforce it: `petri` does not depend on `swab`, so it *cannot*
reach the writer.

**`swab/` stays at `swab/`; the workspace has no `crates/` directory.** Cargo
workspace members need no common parent. `swab/` is referenced roughly thirty times
across `CLAUDE.md`, `README.md`, `ARCHITECTURE.md`, `CONTEXT.md`, `install.sh`,
`pyproject.toml` and its own `scripts/{verify,diff_check,py_probe}` — tidiness there
buys nothing and costs a churn pass over all of it plus the git history of the
scanner port.

**`petridish-core` holds the schema *and* the presentation helpers, not the schema
alone.** The serde types have to be shared — duplicating them is how the contract
drifts, and "single writer" constrains *who writes the file*, not who may reuse the
types. The less obvious half is the presentation layer:
`swab list`'s table and `petri`'s rows independently derive the same four things
from a `Project` — the agent label (`claude-code (working)`), the dirty marker, the
`name (in parent-name)` worktree cell, and the bucket/activity display strings.
Leaving those duplicated means a change to what "working" looks like has to land
twice or the CLI and the TUI disagree about what the same project is doing. So
`present` moves into core and `cli.rs::_print_table` is refactored to call it, with
its existing tests as the safety net.

Column-width computation stays in `cli.rs`: it is genuinely CLI-only, because
ratatui computes its own layout.

We stopped short of moving the grouping/filtering/selection state machine into core.
It would be reusable in principle and testable without ratatui either way, but only
`petri` will ever call it, and "shared crate" would quietly become "petri's guts,
relocated".

**Consequence for installation.** `cargo install --path swab` no longer installs
everything; `petri` needs its own `cargo install --path petri` (or
`cargo install --path . --bins` from the workspace root). `install.sh`'s preflight
check, which currently verifies `swab` and `swab-hook` are on `PATH`, gains `petri`.
Separately and unrelated to the workspace: `install.sh` resolves the venv
interpreter by realpath'ing the `petri` shim — a trick it adopted precisely because
`swab`/`swab-hook` stopped being Python — so that anchor must move to
`petridish-installer`, the last remaining Python console script.
