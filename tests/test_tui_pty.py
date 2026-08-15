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

import json
import os
import re
import select
import sys
import time
from datetime import datetime, timedelta, timezone

import pytest

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
    replayed, so adjacent fields run together. That is fine for substring
    assertions and keeps this helper from becoming a terminal emulator.
    """
    return _ANSI.sub("", raw)


class _Petri:
    """A live ``petri`` in a pty. Use as a context manager."""

    def __init__(self, state_path: str) -> None:
        self.state_path = state_path
        self.pid = -1
        self.fd = -1

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
        return _plain(out.decode("utf-8", "replace"))

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
    """
    with _Petri(state_file) as petri:
        frame = petri.read(_FRAME_TIMEOUT_S)
        assert frame.index("rtk") < frame.index("project-radar") < frame.index("nono")
        assert "⚠" in frame
        stalled_line = next(ln for ln in frame.splitlines() if "⚠" in ln)
        assert "rtk" in stalled_line


def test_z_toggles_density(state_file):
    """Three running projects default to roomy; z must switch to compact.

    Asserted on the *shape of a row* rather than the word "compact": in roomy a
    project's name and its last event are on different lines, and in compact
    they are on the same one. That row is new to the frame, so curses writes it
    whole — see the module docstring on incremental redraw.
    """
    with _Petri(state_file) as petri:
        roomy = petri.read(_FRAME_TIMEOUT_S)
        assert not any(
            "project-radar" in ln and "Edit tui.py" in ln for ln in roomy.splitlines()
        ), "roomy should put the name and the event on separate lines"

        compact = petri.send(b"z", timeout=_FRAME_TIMEOUT_S)
        assert any(
            "project-radar" in ln and "Edit tui.py" in ln for ln in compact.splitlines()
        ), f"z did not collapse to compact rows; got: {compact!r}"


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
