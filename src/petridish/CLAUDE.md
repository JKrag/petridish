# `src/petridish/` — Python read-side

Scope, stack, and the read-only invariant live in the repo-root `CLAUDE.md`. This file
holds context specific to this directory that doesn't belong in `petri/SPEC.md` (the
Rust TUI's spec) or `CONTEXT.md` (the glossary) — the current file layout, `petripy`'s
own status and testing lessons, and the `wcwidth` incident that still shapes a live
constraint on the Rust side. Consolidated here (from `CONTRIBUTING.md`) so this
directory's own contributor-facing history lives with the code it's about.

## Layout

```
schema.py         the wire contract for projects.json — shared by every frontend
tui.py            petripy: curses blitter + key handling — keep it thin
tui_state.py      petripy: pure logic — testable without a terminal
screens.py        petripy: the two petri screens (Radar -> list[str])
menubar.py        pure xbar/SwiftBar renderer (str in, str out) — not petripy, no
                  Rust replacement in scope
installer.py      launchd job + Claude Code hook wiring — not petripy either
```

`scan.py`/`config.py`/`sensors/` (the old Python scanner) are **gone**, not just
deprecated — fully replaced by `swab`/Rust and deleted, per repo-root `CLAUDE.md`. If
you're looking at a reference to one of those files anywhere, it's stale.

The `tui_state.py` / `screens.py` / `tui.py` split follows the rule **push logic into
a pure function so it can be tested without a display** — `screens.py` returns
`list[str]` already clipped to width/height, so `tui.py` owns no layout arithmetic at
all. That's what makes `swab dash` (the same dashboard, printed once,
non-interactively) a three-line function rather than a second renderer.

## Two things about the petripy screens that look wrong and aren't

- **`glyph_for()` ignores `project.agent.state`.** It re-derives the state from
  `last_event_at` at render time instead. The stored field was stamped when the
  daemon last scanned; the glyph sits next to a live silence counter, so reading
  the stale field would let the two disagree on screen. The thresholds that
  define both live in `schema.py` (`agent_state_for_silence`) precisely so they
  cannot drift apart. (The Rust `petri`'s `silence_tier_color`/`glyph_for` make the
  identical choice, for the identical reason — see `petri/src/dashboard.rs`.)
- **The dashboard sorts by *longest* silence first.** Not most-recent-first.
  It is a triage order: the run that has stopped moving is the one you need, and
  freshest-first would bury it under the healthy ones. (Rust equivalent:
  `RUNNING_ATTENTION_CEILING_S` in `petri/src/dashboard.rs` — same idea, with a
  ceiling added after real use showed unbounded quietest-first had its own
  failure mode.)

Also worth knowing, and not petripy-specific: `agent.state` is a pure recency clock,
not a liveness signal. `working` means "emitted an event in the last 90 seconds", so
a local model mid-inference for four minutes reads `recent` — indistinguishable from
a run that wedged four minutes ago. Telling *finished* from *wedged* needs a sensor
this project does not have yet; the stalled-run glyph means "hasn't moved", not
"broken".

## The pty test (petripy)

`tests/test_tui_pty.py` spawns the real `petripy` in a pseudo-terminal and sends
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

The Rust `petri`'s PTY harness (`petri/tests/pty_support/mod.rs`) found a different,
harder set of bugs the same way — draining-thread lifetime and pty-handle-drop races
— documented there and in `petri/SPEC.md` §8 layer 3. Not a coincidence that both
independent PTY harnesses needed hard-won lessons: driving a real terminal
program is just an inherently race-prone thing to test, in either language.

## `petripy`'s lifecycle

`petripy` (`tui.py`/`tui_state.py`/`screens.py`) is `petri`'s deprecated Python/curses
predecessor — see `CONTEXT.md`'s `petripy` entry for what it is and why it's named that.
Status, concretely:

- **Frozen on arrival of the Rust build.** No new features land here. It stays
  installed and its tests stay in `make check` so it cannot rot into a broken fallback
  while still being relied on.
- **Deletion trigger:** remove `tui.py`/`tui_state.py`/`screens.py` and their tests
  after a few weeks of real use on the Rust `petri` — the same pattern the Python
  scanner's deletion followed (see repo-root `CLAUDE.md`'s note on `swab`). Not yet
  triggered as of this writing.
- `menubar.py`, `installer.py`, `schema.py` are unaffected by this — they're not part
  of the deprecated TUI and have no Rust replacement in scope.

## The `wcwidth` incident (canonical account)

This is the founding incident behind `petri/SPEC.md` §4.2's glyph-portability gate, and
the reason `tests/test_glyph_portability.py` exists at all. Keeping the full account in
one place (here) rather than re-told in three (a spec doc, a Rust doc comment, and the
Python test's own docstring) — those other two now just point here.

`petripy` shipped `⚠` (U+26A0, added in Unicode 4.0) as its stalled-run glyph. It
rendered as a **blank cell** on the macOS 14 CI runner: ncurses asks libc's `wcwidth()`
before placing any character, macOS's Unicode tables lag the current standard, and for
a codepoint they don't recognize the answer is `-1` — ncurses substitutes a space rather
than guess. `●` (U+25CF) and `✎` (U+270E), both Unicode 1.1, rendered fine right next to
it on the same screen.

What made it expensive wasn't the rendering bug itself — it's that nothing in a
300-test suite noticed. The stalled-run glyph is, by the dashboard's own design, *the
row you opened the dashboard to find*. It silently vanished, and every substring
assertion still passed, because the project's name also appears on its own path row.
The screen read as a calm, all-clear dashboard while quietly hiding the one thing it
existed to surface.

Fixed by moving to `▲` (U+25B2, Unicode 1.1). Backed by a mechanical gate rather than a
convention: `tests/test_glyph_portability.py` fails any non-ASCII character introduced
into a curses-rendered module (`CURSES_MODULES` — `tui.py`/`tui_state.py`/`screens.py`)
until it's added to that file's `ALLOWED` dict with a reason, and a second test verifies
every entry's Unicode-1.1 claim against real codepoint blocks rather than trusting it.
Scope note: it scans whole files, comments and docstrings included — a stray unportable
character in a comment is a loaded gun, one keystroke from being pasted into a real
footer (the `⏎` that used to sit in a `screens.py` footer comment was exactly that).
`menubar.py` is out of scope: SwiftBar renders it, not ncurses, so its 🧫 (Unicode 11.0)
is fine there.

**This is a `wcwidth`/ncurses-specific bug**, not a blanket "no terminal renders modern
Unicode" fact — plenty of terminals and tools (including non-ncurses ones on the very
same macOS box) render `⚠` correctly; they just aren't exercising this codepath. Whether
and how a Unicode-1.1-only allowlist should apply to `petri` (Rust, ratatui, no
`wcwidth` in its dependency tree at all) is a separate question, answered on its own
terms in `petri/SPEC.md` §4.2 and `petri/tests/glyph_portability.rs` — do not assume the
same bar transfers unexamined just because the lesson (an unverified glyph can fail
silently, in the one row that matters, with every test still green) does.
