# `petri` verification: four layers, no parity oracle

**Status:** accepted. Partially superseded by ADR-0004 — see the note below.

> **Note (ADR-0004).** This ADR's framing refers to the Python implementation and
> `swab/scripts/diff_check.sh` in the present tense. Both are gone. The differential
> oracle described below had in fact stopped working long before it was deleted:
> `py_probe.py` imported `petridish.config`/`.discovery`/`.git`/`.sensors.*`, all
> removed when the scanner was ported, so the scripts could not have run at all.
>
> The *decision* this ADR records — four verification layers for `petri`, and
> deliberately no parity oracle — stands, and those layers are what
> `petri/tests/s*_snapshot.rs` and `s*_pty*.rs` still implement. Only the
> description of the alternative is historical.

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

The failure mode is worth naming precisely, because every obvious guess was wrong.
Not locale (ruled out empirically — no locale reproduces the observed signature of
`·`/`═`/`─` present but `⚠` absent; they are all-or-nothing). Not a
partially-painted frame either: the ellipsis in the captured CI frame was pytest's
own repr truncation, not a missing section.

What it actually was: **`tui._put` silently discarded an entire row.** It wrapped
`addnstr` in `except curses.error: pass`, so when one cell refused the write the
whole line vanished. The renderers are handed `width - 1`, leaving exactly one
column of slack against the real window — enough for the ASCII path everywhere,
and consumed by a glyph the terminal reckons as two columns wide. The three
RUNNING headlines dropped, and because each project's name also appears on its
path row, every substring the tests looked for was still present: the frame read
as a calm dashboard. That is the precise failure this whole document exists to
prevent — a screen that silently lies.

Two lessons, both now enforced:

1. **A renderer must degrade to less content, never to no content.** `_put` now
   falls back to placing characters individually and skips only the cells
   actually refused. Fixed in the same change as this ADR's revision.
2. **Assert identity against the renderer, not substrings against a byte stream.**
   The decisive new test reconstructs the terminal grid and compares it row-for-row
   with `render_dashboard`'s own output for the same fixture and geometry —
   `tui.py` is meant to be a dumb blitter, so identity is the honest contract.
   Substring assertions provably could not catch this; the identity test catches it
   on the first missing row and prints both screens.

Building that reconstruction (`tests/test_tui_pty.py::_screen`) also settled two
things the Rust layer would have had to rediscover: ncurses leans heavily on
`\x1b[nG` (absolute column) for these wide rows, and a three-byte box-drawing
character straddles `os.read` boundaries often enough to need an *incremental*
UTF-8 decoder. Both cost a round of wrong output to find.

Hence, for the Rust layer 3: assert against a **settled full-screen snapshot**
(read until the stream quiesces, reconstruct the grid, compare against what the
widget layer intended), inject a fixed clock, set the winsize explicitly, and pin
`LANG`/`LC_ALL`. Any test that cannot be made deterministic that way does not
belong in this layer.

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
unattended agent needs exactly one command that is the entire truth.
