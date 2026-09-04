"""End-to-end test of the real curses TUI, driven through a pseudo-terminal.

Everything else in this suite tests the renderers with the display closed,
which is the right default — but it leaves the parts that only exist inside
curses completely uncovered: the ``curses.wrapper`` wiring, the key-dispatch
branches, the blitter, and whether ``q`` actually gets you your shell back.

So this spawns ``petri`` in a real pty against a fixture ``projects.json``,
sends keystrokes, and asserts on what lands on the screen. It is slow by the
standards of the rest of the suite (a few seconds) and it is the only test here
that can hang, which is why every wait is bounded.

**Two traps, both hit while writing this.**

1. *Keep draining the pty while waiting for the child to exit.* The TUI
   re-renders every ``_POLL_MS``. Stop reading and the pty buffer fills, the
   child blocks in ``write()``, and it never gets round to reading the ``q`` you
   sent. That looks exactly like "the TUI ignores q" and is not.
2. *Set the window size explicitly.* A forked pty starts at 0x0, and the
   renderers are handed ``getmaxyx()`` — at 0x0 they correctly produce nothing,
   so every assertion fails for an uninteresting reason.

**And one thing that shapes every assertion here:** curses repaints only the
cells that *changed*, so an arbitrary literal string is not guaranteed to appear
contiguously in the byte stream. Toggling density really does put ``compact`` on
screen, yet the pty yields ``...quietest first · compacmain   ✎3...`` — the
unchanged cells in between were simply never rewritten. So assert on **whole
rows that are new to the frame**, never on a word that might straddle a cell the
previous frame already had right. Exact output is pinned in ``test_screens.py``,
where the renderer is called directly; this file is about the loop.
"""

from __future__ import annotations

import codecs
import json
import os
import re
import select
import sys
import time
from datetime import datetime, timedelta, timezone

import pytest

from petridish.schema import read_json
from petridish.screens import render_dashboard

pytestmark = pytest.mark.skipif(
    not hasattr(os, "fork") or sys.platform not in ("darwin", "linux"),
    reason="needs pty.fork(); petri is a macOS tool and CI runs macos-14",
)

ROWS, COLS = 40, 100

#: Bounded waits. The TUI polls at 2s, so a frame is always available inside
#: this window; nothing here should ever wait longer than a few seconds.
_FRAME_TIMEOUT_S = 2.5
_KEY_TIMEOUT_S = 1.5
_EXIT_TIMEOUT_S = 6.0


def _iso(dt: datetime) -> str:
    return dt.strftime("%Y-%m-%dT%H:%M:%SZ")


def _fixture_radar(now: datetime) -> dict:
    """A realistic overnight state: two live runs, one stalled, plus the tail."""

    def project(
        name: str,
        bucket: str,
        *,
        silence_s: float | None = None,
        agent: str | None = None,
        dirty: int = 0,
        commit_h: float | None = None,
        event: str | None = None,
        session: str | None = None,
    ) -> dict:
        commit = _iso(now - timedelta(hours=commit_h)) if commit_h else None
        return {
            "id": f"id-{name}",
            "name": name,
            "path": f"{os.path.expanduser('~')}/repos/JKrag/{name}",
            "category": "JKrag",
            "is_foreign": False,
            "status_bucket": bucket,
            "git": {
                "is_repo": True,
                "branch": "main",
                "is_dirty": dirty > 0,
                "uncommitted_files": dirty,
                "last_commit_at": commit,
                "mine_last_commit_at": commit,
                "github_url": f"https://github.com/JKrag/{name}",
            },
            "agent": {
                "state": "idle",
                "active_agent": agent,
                "last_event": event,
                "last_event_at": (
                    _iso(now - timedelta(seconds=silence_s))
                    if silence_s is not None
                    else None
                ),
                "session_id": session,
            },
            "last_activity_at": _iso(now - timedelta(hours=1)),
        }

    return {
        "schema_version": 1,
        "updated_at": _iso(now),
        "scan_duration_ms": 440,
        "projects": [
            project("rtk", "active", silence_s=2820, agent="ornith-35b", dirty=3,
                    event="Bash cargo test", session="2b81f004"),
            project("nono", "active", silence_s=12, agent="claude-opus-5", dirty=2,
                    event="Bash nono promote", session="7f2c1d90"),
            project("project-radar", "active", silence_s=252, agent="ornith-35b",
                    dirty=7, event="Edit tui.py", session="a43ebbd0"),
            project("wip-thing", "in_flight", commit_h=96, dirty=1),
            project("dusty", "stale", commit_h=1000),
            project("ancient", "cold", commit_h=9000),
        ],
    }


_ANSI = re.compile(r"\x1b\[[0-9;?]*[a-zA-Z]|\x1b[()][A-Z0-9]|\x1b[=>]")


def _plain(raw: str) -> str:
    """Strip escape sequences so assertions can be made on visible text.

    Deliberately crude: cursor-positioning codes are removed rather than
    replayed, so adjacent fields run together. Fine for "is this substring
    anywhere on screen", and nothing else — see :func:`_screen` for why.
    """
    return _ANSI.sub("", raw)


_CSI = re.compile(r"\x1b\[([0-9;?]*)([a-zA-Z])")
#: Non-CSI escapes ncurses emits for this app. Length matters: ``\x1b(B`` is
#: three bytes, ``\x1b=`` is two. Skipping a fixed two left a stray ``B``
#: painted onto the grid, which is how a reconstruction quietly acquires
#: characters the program never wrote.
_ESC_OTHER = re.compile(r"\x1b(?:[()][0-9A-Za-z]|[=><78])")


def _screen(raw: str, rows: int = ROWS, cols: int = COLS) -> list[str]:
    """Replay ``raw`` into a character grid and return it as ``rows`` strings.

    **Why this exists.** ``_plain`` throws cursor-positioning codes away, so the
    result's ``splitlines()`` boundaries are wherever ncurses happened to emit a
    ``\r`` or ``\n`` — an artifact of its cursor-movement *optimisation*, not of
    the layout. Any assertion of the form "these two fields are/aren't on the
    same line" made against that stream is really asserting on ncurses' choice
    of escape sequences, and it flips between machines for no reason the test
    can see. Two tests here did exactly that and failed on CI while passing
    locally.

    So: apply the positioning instead of discarding it, and assert on real rows.
    Handles what ncurses actually emits for this app, established by counting
    the sequences in a real session rather than guessed: absolute positioning
    (``H``/``f``), absolute column (``G``, which ncurses uses constantly for
    these wide rows), absolute row (``d``), relative moves (``A``–``D``),
    erase-to-end-of-line (``K``), erase-display (``J``), CR, LF and BS. Colour,
    mode switches, scroll regions and the alternate screen are ignored. It is
    deliberately not a terminal emulator; if a future frame needs scroll regions
    or wide-character cell accounting, that is the point to reach for a real
    one rather than growing this.
    """
    grid = [[" "] * cols for _ in range(rows)]
    row = col = 0

    def clamp() -> None:
        nonlocal row, col
        row = max(0, min(rows - 1, row))
        col = max(0, min(cols - 1, col))

    i = 0
    while i < len(raw):
        ch = raw[i]
        if ch == "\x1b":
            m = _CSI.match(raw, i)
            if m is None:
                other = _ESC_OTHER.match(raw, i)
                i = other.end() if other else i + 2
                continue
            params, final = m.group(1), m.group(2)
            nums = [int(n) for n in params.split(";") if n.isdigit()]
            if final in "Hf":
                row = (nums[0] - 1) if nums else 0
                col = (nums[1] - 1) if len(nums) > 1 else 0
                clamp()
            elif final == "A":
                row -= nums[0] if nums else 1
                clamp()
            elif final == "B":
                row += nums[0] if nums else 1
                clamp()
            elif final == "C":
                col += nums[0] if nums else 1
                clamp()
            elif final == "D":
                col -= nums[0] if nums else 1
                clamp()
            elif final == "G":
                # CHA — absolute column. ncurses leans on this heavily for this
                # app's wide rows; without it every right-hand field collapses
                # leftwards against the field before it.
                col = (nums[0] - 1) if nums else 0
                clamp()
            elif final == "d":
                row = (nums[0] - 1) if nums else 0
                clamp()
            elif final == "K":
                mode = nums[0] if nums else 0
                if mode == 0:
                    for c in range(col, cols):
                        grid[row][c] = " "
                elif mode == 1:
                    for c in range(0, col + 1):
                        grid[row][c] = " "
                else:
                    grid[row] = [" "] * cols
            elif final == "J":
                mode = nums[0] if nums else 0
                if mode == 2:
                    grid = [[" "] * cols for _ in range(rows)]
                elif mode == 0:
                    for c in range(col, cols):
                        grid[row][c] = " "
                    for r in range(row + 1, rows):
                        grid[r] = [" "] * cols
            i = m.end()
            continue
        if ch == "\r":
            col = 0
        elif ch == "\n":
            row += 1
            clamp()
        elif ch == "\b":
            col = max(0, col - 1)
        elif ch == "\x07":
            pass
        else:
            grid[row][col] = ch
            col += 1
            if col >= cols:
                col = cols - 1
        i += 1

    return ["".join(r).rstrip() for r in grid]


def _dump(screen: list[str]) -> str:
    """A readable failure message: the whole reconstructed screen, numbered."""
    return "\n".join(f"{i:3} |{line}|" for i, line in enumerate(screen))


class _Petri:
    """A live ``petri`` in a pty. Use as a context manager."""

    def __init__(self, state_path: str, pinned_now: "datetime | None" = None) -> None:
        self.state_path = state_path
        #: When set, the child renders against this instant instead of the wall
        #: clock (``tui._now``). Only the row-for-row identity test needs it;
        #: every other test here asserts on content that does not move.
        self.pinned_now = pinned_now
        self.pid = -1
        self.fd = -1
        #: Every byte the child has written, kept so :meth:`screen` can replay
        #: the whole session into a grid. A frame is a full repaint, so
        #: replaying everything yields the *latest* screen, not a smear.
        self.raw = ""
        #: Decoding must be *incremental*: a box-drawing character is three
        #: bytes and lands astride an ``os.read`` boundary often enough to see
        #: it. Decoding each chunk independently turns those into U+FFFD.
        self._decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def __enter__(self) -> _Petri:
        import pty

        code = (
            "import petridish.tui as t;"
            f"t._DEFAULT_STATE_PATH={self.state_path!r};"
            "raise SystemExit(t.main())"
        )
        src = os.path.join(os.path.dirname(os.path.dirname(__file__)), "src")
        self.pid, self.fd = pty.fork()
        if self.pid == 0:  # pragma: no cover - child process
            os.environ["TERM"] = "xterm-256color"
            os.environ["PYTHONPATH"] = src
            # Pin the child's clock so a rendered duration or the header clock
            # cannot tick between the renderer call in the parent and the frame
            # the child draws. See `tui._now`.
            if self.pinned_now is not None:
                os.environ["PETRIDISH_NOW"] = self.pinned_now.isoformat()
            os.execvp(sys.executable, [sys.executable, "-c", code])

        # A forked pty starts at 0x0; the renderers would correctly emit nothing.
        import fcntl
        import struct
        import termios

        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        return self

    def read(self, timeout: float) -> str:
        out = b""
        end = time.time() + timeout
        while time.time() < end:
            ready, _, _ = select.select([self.fd], [], [], 0.2)
            if not ready:
                continue
            try:
                chunk = os.read(self.fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            out += chunk
        decoded = self._decoder.decode(out)
        self.raw += decoded
        return _plain(decoded)

    def settle(self, timeout: float = _FRAME_TIMEOUT_S, quiet: float = 0.3) -> list[str]:
        """Read until the child stops writing, then return the screen it left.

        A single timed ``read`` can return mid-repaint, which is how you end up
        asserting against half a frame. This waits for ``quiet`` seconds of
        silence (bounded by ``timeout``) so the grid is a settled screen.
        """
        end = time.time() + timeout
        last_data = time.time()
        while time.time() < end:
            ready, _, _ = select.select([self.fd], [], [], 0.1)
            if ready:
                try:
                    chunk = os.read(self.fd, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                self.raw += self._decoder.decode(chunk)
                last_data = time.time()
            elif time.time() - last_data >= quiet:
                break
        return self.screen()

    def screen(self) -> list[str]:
        """The reconstructed grid for everything written so far."""
        return _screen(self.raw)

    def send(self, keys: bytes, timeout: float = _KEY_TIMEOUT_S) -> str:
        os.write(self.fd, keys)
        return self.read(timeout)

    def wait(self) -> int:
        """Reap an already-exiting child. Returns its exit code, or -1 on kill."""
        end = time.time() + _EXIT_TIMEOUT_S
        while time.time() < end:
            self.read(0.2)
            done, status = os.waitpid(self.pid, os.WNOHANG)
            if done:
                return os.waitstatus_to_exitcode(status)
        os.kill(self.pid, 9)
        os.waitpid(self.pid, 0)
        return -1

    def quit(self) -> int:
        """Send ``q`` and return the exit code, or -1 if it had to be killed."""
        os.write(self.fd, b"q")
        end = time.time() + _EXIT_TIMEOUT_S
        while time.time() < end:
            # Keep draining: a full pty buffer blocks the child's write() so it
            # never reads the 'q'. See the module docstring.
            self.read(0.2)
            done, status = os.waitpid(self.pid, os.WNOHANG)
            if done:
                return os.waitstatus_to_exitcode(status)
        os.kill(self.pid, 9)
        os.waitpid(self.pid, 0)
        return -1

    def __exit__(self, *_exc) -> None:
        try:
            os.kill(self.pid, 9)
            os.waitpid(self.pid, 0)
        except (OSError, ChildProcessError):
            pass
        try:
            os.close(self.fd)
        except OSError:
            pass


@pytest.fixture
def state_file(tmp_path) -> str:
    path = tmp_path / "projects.json"
    path.write_text(json.dumps(_fixture_radar(datetime.now(timezone.utc))))
    return str(path)


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

def test_petri_renders_the_dashboard_and_quits_cleanly(state_file):
    """The whole loop: curses.wrapper wiring, a real frame, and ``q``.

    Nothing else in the suite reaches ``curses.wrapper`` at all.
    """
    with _Petri(state_file) as petri:
        frame = petri.read(_FRAME_TIMEOUT_S)
        assert "petri · dashboard" in frame
        assert "RUNNING" in frame
        assert "rtk" in frame
        assert petri.quit() == 0, "petri did not exit cleanly on 'q'"


def test_dashboard_orders_by_silence_and_flags_the_stalled_run(state_file):
    """The design's central claim, asserted against a real terminal.

    rtk (silent 47m) must be the first card and must carry the warning glyph;
    nono (silent 12s) must not.

    Asserted on the reconstructed grid (:func:`_screen`), not the byte stream.
    The previous version read "is the glyph anywhere in the output" and "which
    stream segment is it in", and failed on CI for a reason neither assertion
    could show: a card's whole headline row had been silently dropped by
    ``tui._put``, and every project name also appears on its path row, so the
    frame still looked plausible. A headline row that vanishes is now a visible
    failure with the screen printed next to it.
    """
    with _Petri(state_file) as petri:
        screen = petri.settle()
        dump = _dump(screen)

        headlines = [ln for ln in screen if "silent" in ln and "·" in ln]
        assert len(headlines) == 3, f"expected 3 card headlines, got {len(headlines)}\n{dump}"

        names = [ln for ln in screen if any(n in ln for n in ("rtk", "project-radar", "nono"))]
        order = [next(n for n in ("rtk", "project-radar", "nono") if n in ln) for ln in names]
        # Quietest first: rtk (47m), project-radar (4m), nono (12s). Each project
        # occupies several rows, so compare first appearances.
        first = [n for i, n in enumerate(order) if n not in order[:i]]
        assert first == ["rtk", "project-radar", "nono"], f"wrong order: {first}\n{dump}"

        stalled = [ln for ln in screen if "▲" in ln]
        assert len(stalled) == 1, f"expected exactly one ▲ row, got {len(stalled)}\n{dump}"
        assert "rtk" in stalled[0], f"▲ is not on rtk's row\n{dump}"
        assert not any("nono" in ln and "▲" in ln for ln in screen), dump


def test_blitter_puts_the_renderer_output_on_screen_verbatim(state_file):
    """Every row the renderer produced arrives on screen unchanged.

    This is the check the CI failure needed and nobody had written. ``tui.py``
    is meant to be a dumb blitter — ``screens.py`` decides every column, and
    ``_blit``/``_put`` only copy. So the honest assertion is identity, not
    substrings: reconstruct the terminal and compare it row-for-row against
    what ``render_dashboard`` returned for the same fixture and geometry.

    Substring assertions could not catch what actually went wrong (``_put``
    silently discarding a whole row when one cell refused the write) because
    every project's name also appears on its path row, so the frame still
    contained every string the tests looked for. Identity catches it on the
    first row that goes missing, and prints both screens.
    """
    # One instant for both sides. Without this the parent renders at T1 and the
    # child draws at T2, and any duration on screen ticks between them: a real
    # run of this test failed on `silent 13s` vs `silent 12s`, a correct program
    # reported as a defect. petri/SPEC.md §8 requires an injected clock for the
    # Rust PTY layer for exactly this reason; this suite predates that rule.
    now = datetime.now(timezone.utc)
    expected = render_dashboard(
        read_json(state_file),
        now=now,
        width=COLS - 1,
        height=ROWS,
        home=os.path.expanduser("~"),
    )
    with _Petri(state_file, pinned_now=now) as petri:
        actual = petri.settle()

    for i, exp in enumerate(expected):
        got = actual[i] if i < len(actual) else ""
        assert exp.rstrip() == got.rstrip(), (
            f"row {i} differs between renderer and screen\n"
            f"  renderer |{exp.rstrip()}|\n"
            f"  screen   |{got.rstrip()}|\n\n"
            f"full screen:\n{_dump(actual)}"
        )


def test_z_toggles_density(state_file):
    """Three running projects default to roomy; z must switch to compact.

    Asserted on the *shape of a row*: in roomy a project's name and its last
    event are on different screen rows, in compact they share one.

    This assertion used to be made against ``_plain``'s output split on
    newlines, which meant it was really asserting on where ncurses chose to
    emit a ``\r`` while optimising cursor movement — it passed locally and
    failed on CI with nothing in the diff to explain why. :func:`_screen`
    replays the positioning so "same row" means the same row.
    """
    with _Petri(state_file) as petri:
        roomy = petri.settle()
        assert not any(
            "project-radar" in ln and "Edit tui.py" in ln for ln in roomy
        ), f"roomy should put the name and the event on separate rows\n{_dump(roomy)}"

        os.write(petri.fd, b"z")
        compact = petri.settle()
        assert any(
            "project-radar" in ln and "Edit tui.py" in ln for ln in compact
        ), f"z did not collapse to compact rows\n{_dump(compact)}"


def test_tab_switches_screens_both_ways(state_file):
    with _Petri(state_file) as petri:
        petri.read(_FRAME_TIMEOUT_S)
        assert "browser" in petri.send(b"\t")
        assert "dashboard" in petri.send(b"\t")


def test_browser_moves_the_cursor_and_shows_the_detail_pane(state_file):
    with _Petri(state_file) as petri:
        petri.read(_FRAME_TIMEOUT_S)
        petri.send(b"\t")
        frame = petri.send(b"j")
        assert "sess" in frame, "detail pane did not render for the selected row"


def test_browser_search_filters_the_list(state_file):
    with _Petri(state_file) as petri:
        petri.read(_FRAME_TIMEOUT_S)
        petri.send(b"\t")
        frame = petri.send(b"/nono\r", timeout=_FRAME_TIMEOUT_S)
        assert "nono" in frame
        assert "dusty" not in frame


def test_petri_reports_a_missing_state_file_without_starting_curses(tmp_path):
    """The pre-curses bail-out, exercised as a real process.

    ``test_tui.py`` covers this by monkeypatching; this proves the packaged
    entry point behaves the same way when actually run.
    """
    missing = str(tmp_path / "nope.json")
    with _Petri(missing) as petri:
        out = petri.read(2.0)
        # No 'q' here: the process has already exited, and writing to the pty of
        # a dead child raises EIO. Just reap it.
        code = petri.wait()
    assert "run 'swab scan' first" in out
    assert code == 1
