# Contributing to petridish

Written for whoever picks this up next — human or AI agent. It says what to run,
what must not break, and which mistakes this codebase has already made so you
don't repeat them.

## The one command

```sh
make check
```

That is the whole gate: tests, the type ratchet, and the Python-version floor
check. CI runs the identical command, so a green `make check` locally means a
green CI run. Run it before proposing any change.

```
make install    # uv sync --extra dev
make test       # pytest
make typecheck  # pyright, ratcheted (see below)
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

These are in `CLAUDE.md` in full. The ones that have actually been violated in
practice:

1. **Single writer.** Only the daemon writes `projects.json`, via temp file +
   `os.replace()`. `swab-hook` appends one line to `events.ndjson` and nothing
   else — three other hook consumers share those events.
2. **Never parse a path out of a `~/.claude/projects/` dirname.** The slug
   encodes `/` and `-` identically and is not reversible. Read `cwd` from the
   JSONL contents.
3. **`cwd` varies within one transcript.** Take the *last* parseable line, then
   run it through `resolve_root()` so monorepo subdirs collapse to one project.
4. **Truncated trailing JSONL lines are normal**, not errors — live sessions are
   being appended to as you read.
5. **Degrade, never abort.** A failing sensor yields `null` fields and the tick
   still writes a complete file. This one is subtle: a bad *config value* used
   to raise inside `load_config()`, get swallowed by the broad `except` in
   `cli.py`, and take out the entire tick — `projects.json` silently froze with
   only a daemon-log line. Config values now fall back per-key and warn on
   stderr.
6. **`git` calls** use `subprocess.run(check=False)` with a 5s timeout. A git
   failure is `GitState(is_repo=False)`, never an exception.

## Zero runtime dependencies

`src/petridish/` is **stdlib only**. No `watchdog`, no `pydantic`, no `click`,
no `rich`. This is deliberate: it makes every module verifiable with no
environment setup, and it's why the daemon needs nothing but a Python 3.12.

`pytest` and `pyright` are dev-only and don't violate this. GUI clients
(`raycast/`, and any future menu-bar host) are separate projects that only ever
*read* `~/.petridish/projects.json` — they may use whatever they like, and they
never link against the core.

## Testing

Real fixtures, not mocks. `git init` actual repos in tmpdirs with pinned author
and date env vars; write actual fixture transcript files. Mocked subprocess
output would have hidden most of the findings this project is built on.

**Tests must be hermetic.** `tests/test_scan.py` has an autouse `_hermetic_home`
fixture that redirects `HOME`; never read the real `~/.claude`.

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

## The pty test

`tests/test_tui_pty.py` spawns the real `petri` in a pseudo-terminal and sends
keystrokes. It is the only coverage of `curses.wrapper`, the key dispatch, the
blitter, and whether `q` gives you your shell back — and it costs ~30s of the
suite's runtime. Worth it; don't delete it.

Three things it will teach you the hard way if you extend it:

- **Keep draining the pty while waiting for the child to exit.** The TUI
  repaints every 2s; stop reading and the buffer fills, the child blocks in
  `write()`, and it never reads the `q` you sent. That looks exactly like a TUI
  bug and is not.
- **Set the window size.** A forked pty starts at 0x0, where the renderers
  correctly emit nothing.
- **Never assert on an arbitrary literal string.** curses repaints only changed
  cells, so `compact` really is on screen while the byte stream reads
  `...quietest first · compacmain   ✎3...`. Assert on whole rows that are new to
  the frame. Exact output belongs in `test_screens.py`, where the renderer is
  called directly.

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
src/petridish/          core daemon — stdlib only
  schema.py             the wire contract for projects.json (start here)
  scan.py               discovery + sensor fusion -> Radar
  config.py             TOML loading with per-key fallback
  sensors/              claude.py, copilot.py
  tui_state.py          pure logic for petri — testable without a terminal
  screens.py            the two petri screens (Radar -> list[str])
  tui.py                curses blitter + key handling — keep it thin
  menubar.py            pure xbar/SwiftBar renderer (str in, str out)
scripts/                check_pyver.py, typecheck_ratchet.py
raycast/                separate MIT-licensed extension (core stays GPL)
```

The `tui_state.py` / `screens.py` / `tui.py` split and `menubar.py` follow the
same rule: **push logic into a pure function so it can be tested without a
display.** If you add a frontend, do the same.

`screens.py` returns `list[str]` already clipped to width and height, so
`tui.py` owns no layout arithmetic at all. That is what makes `swab dash` — the
same dashboard, printed once, non-interactively — a three-line function rather
than a second renderer.

## Two things about the petri screens that look wrong and aren't

- **`glyph_for()` ignores `project.agent.state`.** It re-derives the state from
  `last_event_at` at render time instead. The stored field was stamped when the
  daemon last scanned; the glyph sits next to a live silence counter, so reading
  the stale field would let the two disagree on screen. The thresholds that
  define both live in `schema.py` (`agent_state_for_silence`) precisely so they
  cannot drift apart.
- **The dashboard sorts by *longest* silence first.** Not most-recent-first.
  It is a triage order: the run that has stopped moving is the one you need, and
  freshest-first would bury it under the healthy ones.

Also worth knowing: `agent.state` is a pure recency clock, not a liveness
signal. `working` means "emitted an event in the last 90 seconds", so a local
model mid-inference for four minutes reads `recent` — indistinguishable from a
run that wedged four minutes ago. Telling *finished* from *wedged* needs a
sensor this project does not have yet; the `⚠` glyph means "hasn't moved", not
"broken".
