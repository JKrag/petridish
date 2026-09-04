# Contributing to petridish

Written for whoever picks this up next — human or AI agent. It says what to run,
what must not break, and which mistakes this codebase has already made so you
don't repeat them.

## The one command

```sh
make check
```

That is the whole gate: Python tests, the Rust workspace tests, the type ratchet, and
the Python-version floor check. CI runs the identical command, so a green `make check`
locally means a green CI run. Run it before proposing any change, in either language.

```
make install    # uv sync --extra dev
make test       # pytest
make rust-test  # cargo test --workspace -- --test-threads=1
make typecheck  # pyright, ratcheted (see below) — Python only, no Rust equivalent yet
make pyver      # fail if src/ uses stdlib APIs newer than requires-python
make check      # all of the above — this is the gate
```

The gates in `check` are **prerequisites, not one recipe line**. That is
load-bearing: a recipe like `pytest -q; pyright` returns only the *last*
command's exit status, so failing tests would report success. Don't "simplify"
it.

## The type ratchet

`pyright` runs in strict mode. There are still a few hundred pre-existing
diagnostics under `src/`, because the annotations predate anything checking
them. So the gate is **directional, not absolute**: the count may fall, never
rise. It lives in `typecheck-baseline.txt`.

When you improve a file:

```sh
uv run --extra dev python3 scripts/typecheck_ratchet.py --update
```

and commit the smaller number **alongside the fix**. Treat that file as a debt
counter, not a target to game.

`tests/` is deliberately excluded from the count — pytest's `tmp_path`,
`monkeypatch` and `capsys` are untyped at the call site, which produces about a
thousand diagnostics with no signal in them. See the `executionEnvironments`
block in `pyproject.toml`.

## Non-negotiable invariants

Full list, and which crate/module each one currently binds: `CLAUDE.md`. Not
restated here — the two drifted out of sync once before (this file still described
`subprocess.run(check=False)` git calls after `swab/src/git.rs` moved to in-process
`gix`, with no "superseded" note), which is exactly the failure mode a single
source of truth avoids. If you're fixing an invariant violation, read it there.

## Zero runtime dependencies

`src/petridish/` is **stdlib only**. No `watchdog`, no `pydantic`, no `click`,
no `rich`. This is deliberate: it makes every module verifiable with no
environment setup, and it's why the daemon needs nothing but a Python 3.12.
(This never bound the Rust crates — `swab`/`petridish-core`/`petri` take
dependencies freely, pinned in their own `Cargo.toml`s. It's also a distribution
asset for this stdlib-only side specifically, not just a testability one —
`ARCHITECTURE.md` §8.2.)

`pytest` and `pyright` are dev-only and don't violate this. GUI clients
(`raycast/`, and any future menu-bar host) are separate projects that only ever
*read* `~/.petridish/projects.json` — they may use whatever they like, and they
never link against the core.

## Testing

Real fixtures, not mocks. `git init` actual repos in tmpdirs with pinned author
and date env vars; write actual fixture transcript files. Mocked subprocess
output would have hidden most of the findings this project is built on.

**Tests must be hermetic.** Nothing here should read the real `~/.claude` or
`~/.petridish`. `installer.py` is the one module that legitimately touches real
`$HOME`-rooted paths (`~/.claude/settings.json`, `~/Library/LaunchAgents`), so
`tests/test_installer.py` fakes `HOME` per-test rather than relying on a shared
autouse fixture — see its own fixtures for the pattern. The Rust side does the
equivalent with real `git init` repos under `tempdir()`, never the real filesystem.

### Write tests that can fail

The highest-value habit in this repo, and the one that has caught the most
damage:

> **Before trusting a new test, run it against the *unfixed* code.**
> If it passes there, it proves nothing.

This is not hypothetical. Real examples from this codebase:

- A batch of tests for `run_scan`'s defaulting was written, passed, and was
  **reverted** — reintroducing the exact bug they targeted left all of them
  green. The coverage already existed elsewhere.
- A test used `next(line for line in lines if "--repo")`. That condition is a
  truthy string constant, so it always matched the *first* line rather than the
  one under test. It passed while asserting nothing.
- A filter used `"--" in line`, which also matched the `---` separators.

A quick way to check a whole module: mutate an operator (`max`→`min`, flip a
comparison, add an off-by-one) and confirm the suite goes red.

Two mutations that survived while building the petri screens, both because the
fixtures never separated two fields that are usually set together:

- `is_dirty` was always derived from `uncommitted_files > 0`, so a renderer that
  checked only one of them passed. `has_agent` had the same problem with
  `last_event_at` and `session_id`.
- A sort key with an explicit group rank *plus* a negated second element:
  the sign silently did the partitioning, so removing the rank changed nothing
  and no test could tell. Two partitions concatenated is both clearer and
  actually testable.

If a mutation survives, the fix is a test, not a shrug.

**If you script that, clear `__pycache__` between iterations.** Python's
bytecode staleness check compares the source mtime at 1-second granularity, so
writing a mutation and restoring it inside the same second leaves the *mutated*
`.pyc` in place — later runs then execute code that matches nothing on disk.
This has already produced one phantom failure here: a test failed against a
`src/` tree that `git diff` reported as identical to HEAD, for ten confusing
minutes.

```sh
find . -name __pycache__ -type d -not -path "./.venv/*" -exec rm -rf {} +
```

## The pty test (petripy)

Moved to `src/petridish/CLAUDE.md` — `tests/test_tui_pty.py` is `petripy`-specific
(the Rust `petri`'s own PTY-test lessons, a different set found the hard way a second
time, live in `petri/SPEC.md` §8 layer 3 instead).

## Things that look like bugs and aren't

- `from_dict()` takes `Mapping[str, Any]` while `to_dict()` returns a
  `TypedDict`. That asymmetry is deliberate — we guarantee the shape we *write*,
  but what we *read* is whatever is on disk, possibly hand-edited, truncated, or
  from an older schema version. Typing the input strictly would be a lie.
- `scripts/check_pyver.py` tokenizes modules rather than grepping, because a
  docstring explaining why an API is avoided used to match itself. Don't replace
  it with a grep. **Don't delete it either** — it guards against 3.13+/3.14-only
  stdlib APIs that have shipped as real bugs here twice.

## Layout

```
src/petridish/          Python read-side — stdlib only (see src/petridish/CLAUDE.md
                         for the full file-by-file breakdown and petripy specifics)
swab/, petridish-core/, petri/   Rust — scanner, shared schema/present helpers, TUI
scripts/                check_pyver.py, typecheck_ratchet.py
raycast/                separate MIT-licensed extension (core stays GPL)
```

The `tui_state.py` / `screens.py` / `tui.py` split (petripy) and `menubar.py` follow
the rule **push logic into a pure function so it can be tested without a display**;
`petri` (Rust) keeps the same split (`petri/SPEC.md` §1). If you add a frontend, do
the same.

## Two things about the petri screens that look wrong and aren't (petripy)

Moved to `src/petridish/CLAUDE.md` — this is about `glyph_for()`/sort order in the
Python `petripy` specifically. The Rust `petri`'s equivalent decisions
(`RUNNING_ATTENTION_CEILING_S`, `silence_tier_color`'s render-time derivation) are
documented in-place in `petri/src/dashboard.rs`'s own doc comments instead.

### The quota sensor reads someone else's file

The Python `sensors/quota.py` this section used to describe was deleted when the
scanner moved to Rust (`CLAUDE.md`). Its lessons (reject `bool` before `int` —
`bool` is an `int` subclass, so a truthy percentage would otherwise render as 1% —
and drop timestamps more than 30 days out, the classic seconds/milliseconds
mixup) live on, restated for the actual current code, in
`swab/src/sensors/quota.rs`'s own module doc comment. Read there, not here.
