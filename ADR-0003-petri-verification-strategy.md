# `petri` verification: four layers, no parity oracle

**Status:** accepted

The Python→Rust scanner port was verified against a differential oracle:
`swab/scripts/diff_check.sh` ran both implementations and compared output, which is
what made a from-scratch rewrite of `swab` safe to trust. The obvious move is to do
the same for `petri`. We are deliberately not doing that, and this records why plus
what replaces it.

**No differential oracle against petripy.** The scanner port had a property `petri`
does not: byte-for-byte equality with the Python version was the *goal*. Here it is
the opposite — petripy's rendering is shaped by curses, which offers `addstr` and
nothing else, so every screen is a pure function returning pre-padded `list[str]`.
The Rust build discards that layer entirely in favour of ratatui's layout engine,
and changes real behaviour on purpose (the Dashboard gains a cursor, sections become
collapsible, the Browser scrolls instead of truncating). Against an oracle, every
one of those improvements reports as a failure, and the pressure would be to
reproduce curses-era layout decisions to keep the diff clean. The parity we want is
at the level of *what the screen can tell you*, and no oracle checks that.

**What replaces it: four layers, because the work is intended to run unattended.**
This is being built via `delegate-afk`, so the bar is not "a human will look at it"
— it is "the machine can distinguish done from compiles".

1. **Pure-state unit tests.** The derivation layer *is* carried across from Python,
   so its ~55KB of existing pytest cases are ported as test *cases* (not code):
   grouping, bucket membership, worktree rollup, quietest-first ordering, silence
   seconds, humanised durations, selection movement across section boundaries,
   clamping at both ends, the empty selection, re-filtering out the selected row.
   This is the cheap, exhaustive layer.
2. **`TestBackend` buffer snapshots** at 80×24, 200×50 and 40×10, per ratatui's own
   contributing guidance that a `TestBackend` assertion is the most valuable test
   you can write against it. These are self-referential goldens — committed
   expected buffers for *this* implementation — not comparisons with petripy. They
   exist because the thing most likely to silently break when a widget is added is
   the layout, not the derivation, and that is invisible to layer 1.
3. **PTY end-to-end tests** driving the real binary. `tests/test_tui_pty.py` already
   proves this layer earns its keep: it is the only thing covering whether `q`
   actually gives you your shell back, and it caught two traps worth pre-empting
   (stop draining the pty and the child blocks in `write()`, which looks exactly
   like the TUI ignoring your keystroke; a forked pty starts at 0×0, at which point
   every assertion fails for an uninteresting reason).
4. **Human smoke test** — retained, but reclassified as confirmation rather than the
   gate. The intended shape of it is "it works, and I have an idea for a change".

**Layer 3 must be deterministic by construction, and this is the sharp edge.** Two
of the Python PTY tests pass locally and failed on the macOS CI runner — one on a
missing `⚠` glyph, one on a line-split assertion. A flaky layer is worse than no
layer when nobody is watching: an unattended agent cannot tell a flake from a defect,
so it either halts on a false failure or learns to ignore the layer.

The failure mode is worth naming precisely, because the obvious guesses are wrong.
It is not locale. The captured CI frame was
`petri · dashboard … 6 projects … ════ … ──── … tab browser z density q quit` — the
entire RUNNING section absent, i.e. a **partially-painted frame**. Both assertions
are consistent with that: the `⚠` row was never painted, and the other test reads
"lines" out of the raw pty byte stream, where segments are not screen rows at all
under curses' incremental redraw (a trap `test_tui_pty.py`'s own docstring warns
about). Compounding it, `⚠` is derived from silence at *render* time by design —
`glyph_for` bypasses the stored `agent.state` so the glyph cannot disagree with the
live counter beside it — while the fixture builds offsets from `datetime.now()`, so
the assertion is wall-clock-dependent too.

Hence, for the Rust layer 3: assert against a **settled full-screen snapshot**
(read until the stream quiesces, reconstruct the screen, then assert), inject a
fixed clock, set the winsize explicitly, and pin `LANG`/`LC_ALL`. Any test that
cannot be made deterministic that way does not belong in this layer.

(Status of those two tests: red as of the last push, 2026-08-15. They pass locally
on the current tree, which has twelve commits since, so they are not necessarily a
live failure — but they are the reason this layer gets designed rather than copied.)

**Fixtures are authored deliberately, and the nasty one is the point.** The current
corpus is a single-project `projects.golden.json`, which cannot exercise layout at
all. It is replaced by four committed fixtures — minimal, ~15-project normal,
~70-project loaded, and a hostile one (empty project list, all-cold, non-repo, null
branch, 200-char name, CJK/emoji name, absent quota, `updated_at` three days old, a
worktree whose parent is absent, a `schema_version` from the future). We considered
an anonymised snapshot of the real machine's `projects.json` for a guaranteed-real
distribution, and a programmatic builder for expressiveness; both are worse for
layer 2, where a failing snapshot diff has to be readable by eye. `hostile.json` is
where an unattended agent's bugs actually surface, and every layer-1 and layer-2
test runs against it.

**`swab` has never been in CI.** `ci.yml` is Python-only — there is no `cargo test`
in it or in the `Makefile`. That gap predates this work but is closed by it:
`make check` grows `cargo test --workspace` and CI grows a Rust job, because an
unattended agent needs exactly one command that is the entire truth. (Noted while
writing this: CI on master-as-pushed is red, on the two PTY tests described above.)
