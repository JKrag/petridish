"""Interactive TUI for the petridish project radar.

Two screens, toggled with ``tab``:

* **dashboard** — the ambient monitor. Which unattended runs are moving, which
  has gone quiet, what is merely in flight. No cursor, no selection.
* **browser** — the driveable list, with a detail pane and search.

All layout lives in :mod:`petridish.screens`, which returns ``list[str]``
already clipped to the terminal's width and height. This module therefore does
three things and nothing else: read the file, translate keys into state, and
blit lines. Every piece of arithmetic that used to live here — reserved rows for
the detail panel, column widths, section budgets — moved into a pure function
that can be tested with the display closed.

The entry point :func:`main` never calls ``sys.exit`` itself; tests call it
directly and assert on the returned exit code.
"""

from __future__ import annotations

import curses
import os
import sys
from datetime import datetime, timezone

from petridish.schema import _DEFAULT_STATE_PATH, Radar, read_json
from petridish.screens import browser_groups, render_browser, render_dashboard
from petridish.tui_state import (
    SelectionState,
    dashboard_density,
    is_stale,
    move,
    selected_project,
)

#: How often getch() gives up and returns -1, in milliseconds. This is what
#: makes the mtime-poll auto-refresh actually run without a keypress — a
#: blocking getch() (the default) never returns, so the reload check at the
#: top of the loop only ran when the user happened to press a key.
#:
#: It is also what makes the dashboard's silence counters tick: the loop body
#: re-renders unconditionally on every timeout, and the ages are derived at
#: render time from ``last_event_at``. No extra mechanism, no extra scan.
_POLL_MS = 2000

#: The two screens, in ``tab`` order.
_SCREENS = ("dashboard", "browser")

#: Glyphs that carry agent state, mapped to the colour names
#: :func:`_init_color_attrs` provides. Kept here rather than imported so the
#: blitter's concern (which cell to paint) stays separate from the renderer's
#: (what the glyph means).
_GLYPH_COLORS = {"▲": "warn", "●": "green", "○": "dim"}


def _init_color_attrs() -> dict[str, int]:
    """Best-effort color pairs for the agent-state glyphs.

    Returns an empty dict (all glyphs render in the terminal's default
    color) if the terminal doesn't support color — this is cosmetic, never
    a reason to fail the whole TUI.
    """
    try:
        curses.start_color()
        curses.use_default_colors()
        curses.init_pair(1, curses.COLOR_GREEN, -1)
        curses.init_pair(2, curses.COLOR_YELLOW, -1)
        curses.init_pair(3, curses.COLOR_WHITE, -1)
        curses.init_pair(4, curses.COLOR_RED, -1)
        return {
            "green": curses.color_pair(1) | curses.A_BOLD,
            "yellow": curses.color_pair(2) | curses.A_BOLD,
            "dim": curses.color_pair(3) | curses.A_DIM,
            "warn": curses.color_pair(4) | curses.A_BOLD,
        }
    except curses.error:
        return {}


# ---------------------------------------------------------------------------
# Pure helpers (no curses, testable without a terminal)
# ---------------------------------------------------------------------------

def _format_stale_banner(radar: Radar, *, now: datetime | None = None) -> str:
    """Return a banner string describing how old ``radar``'s data is.

    The TUI renders this banner at the top of the screen when
    :func:`is_stale` reports ``True``; the text is built in a tiny pure
    helper so the wording is easy to pin in tests without spinning up a real
    terminal.
    """
    if now is None:
        now = datetime.now(timezone.utc)
    age_hours = (now - radar.updated_at).total_seconds() / 3600.0
    return f"[data is {age_hours:.1f}h old, run 'swab scan' to refresh]"


def find_glyph(line: str) -> tuple[int, str] | None:
    """Locate the agent-state glyph in ``line``, if it has one.

    Returns ``(column, color_name)``. Only the first few columns are searched:
    the glyph is always at the start of a project row (after an optional
    selection marker), and a ``●`` appearing inside a commit message or branch
    name further along the line must not be repainted.
    """
    for i, ch in enumerate(line[:4]):
        color = _GLYPH_COLORS.get(ch)
        if color is not None:
            return (i, color)
    return None


def next_screen(current: str) -> str:
    """The screen ``tab`` moves to. Wraps, so ``tab`` alone cycles both ways."""
    return _SCREENS[(_SCREENS.index(current) + 1) % len(_SCREENS)]


def toggle_density(current_override: str | None, n_rows: int) -> str:
    """What ``z`` should set the density override to.

    Returns the *opposite* of what is currently on screen rather than cycling a
    fixed list, so the first press always visibly changes something — whether
    the current density came from the automatic rule or from a previous press.
    """
    showing = dashboard_density(n_rows, override=current_override)
    return "roomy" if showing == "compact" else "compact"


# ---------------------------------------------------------------------------
# Curses render loop
# ---------------------------------------------------------------------------

def _run(stdscr) -> int:
    """The curses render loop. Called inside ``curses.wrapper``."""
    curses.curs_set(0)  # hide the text cursor
    stdscr.timeout(_POLL_MS)  # getch() returns -1 after this many ms of no input
    color_attrs = _init_color_attrs()
    home = os.path.expanduser("~")

    state_path = _DEFAULT_STATE_PATH
    radar = read_json(state_path)
    last_mtime = os.stat(state_path).st_mtime

    screen = "dashboard"
    density_override: str | None = None
    query = ""
    selection = SelectionState()
    search_mode = False
    search_buffer = ""

    while True:
        try:
            # ---- reload if projects.json changed on disk ----
            try:
                current_mtime = os.stat(state_path).st_mtime
            except OSError:
                # File vanished while TUI was running — bail cleanly.
                return 1
            if current_mtime != last_mtime:
                radar = read_json(state_path)
                selection = SelectionState()
                last_mtime = current_mtime

            now = datetime.now(timezone.utc)
            stdscr.erase()
            h, w = stdscr.getmaxyx()

            # The stale banner costs one row off the top; the renderers are told
            # the reduced height so they budget against what is actually left.
            banner = _format_stale_banner(radar, now=now) if is_stale(radar, now=now) else ""
            top = 0
            if banner:
                _put(stdscr, 0, 0, banner, w, curses.A_BOLD)
                top = 1

            if screen == "dashboard":
                lines = render_dashboard(
                    radar,
                    now=now,
                    width=w - 1,
                    height=h - top,
                    density=dashboard_density(
                        _running_count(radar), override=density_override
                    ),
                    home=home,
                )
            else:
                groups = browser_groups(radar, query)
                lines = render_browser(
                    radar,
                    now=now,
                    width=w - 1,
                    height=h - top,
                    selected=selected_project(selection, groups),
                    query=search_buffer if search_mode else query,
                    home=home,
                )

            _blit(stdscr, lines, top=top, width=w, color_attrs=color_attrs)
            stdscr.refresh()

        except curses.error:
            # One corrupted frame (resize, write-off-edges, …): skip it and
            # retry on the next tick rather than crashing the whole TUI.
            pass

        # ---- input ----
        ch = stdscr.getch()

        if search_mode:
            if ch == -1:
                continue
            if ch in (curses.KEY_ENTER, 10, 13):
                query = search_buffer
                search_mode = False
                search_buffer = ""
                selection = SelectionState()
            elif ch == 27:  # Esc
                search_mode = False
                search_buffer = ""
            elif ch in (curses.KEY_BACKSPACE, 127, 8):
                search_buffer = search_buffer[:-1]
            elif 32 <= ch < 127:
                search_buffer += chr(ch)
            continue

        if ch == -1:
            continue
        if ch in (ord("q"), ord("Q")):
            return 0
        if ch == ord("\t"):
            screen = next_screen(screen)
        elif screen == "dashboard":
            if ch in (ord("z"), ord("Z")):
                density_override = toggle_density(
                    density_override, _running_count(radar)
                )
        else:
            if ch in (curses.KEY_UP, ord("k")):
                selection = move(selection, -1, browser_groups(radar, query))
            elif ch in (curses.KEY_DOWN, ord("j")):
                selection = move(selection, 1, browser_groups(radar, query))
            elif ch == ord("/"):
                search_mode = True
                search_buffer = ""
            elif ch == 27:  # Esc clears an active filter
                query = ""
                selection = SelectionState()


def _running_count(radar: Radar) -> int:
    """How many rows the dashboard's top section will hold.

    The density rule keys on this, and it must match what the renderer will
    actually emit — hence the same ``browser_groups`` filtering rather than a
    count off ``radar.projects``.
    """
    return len(browser_groups(radar)["active"])


def _put(stdscr, y: int, x: int, text: str, width: int, attr: int = 0) -> None:
    """Write one clipped string; degrade to a *shorter* row, never to no row.

    ``addnstr`` raises when a cell refuses the write — writing to the last cell
    of the last line is the textbook case, but a glyph whose display width the
    terminal reckons as 2 does it too, by consuming the one column of slack
    between the renderers' ``width - 1`` line length and the real window width.

    Swallowing that error used to discard the **entire line**. That is the one
    thing this screen must never do: every card's headline carries its name and
    its ``▲``/``●`` state glyph, so a dropped headline doesn't look like a bug,
    it looks like a calm dashboard. (Observed for real: on CI's ncurses the
    three RUNNING headlines vanished and the frame still read as plausible,
    because each project's name also appears on its path row.)

    So on failure, fall back to placing characters individually and skip only
    the ones that are actually refused. A row that cannot fit comes out short;
    it does not come out empty.
    """
    if y < 0 or x < 0 or width - x <= 0:
        return
    avail = width - x - 1
    if avail <= 0:
        return

    def write(col: int, chunk: str, limit: int) -> bool:
        """One write; False if the terminal refused it. Single call site on
        purpose — ``stdscr`` is untyped, so each distinct member access is its
        own pyright diagnostic, and the fallback below would otherwise cost a
        gratuitous point on the typecheck ratchet."""
        try:
            stdscr.addnstr(y, col, chunk, limit, attr)
            return True
        except curses.error:
            return False

    if write(x, text, avail):
        return
    for offset, ch in enumerate(text[:avail]):
        write(x + offset, ch, 1)


def _blit(
    stdscr, lines: list[str], *, top: int, width: int, color_attrs: dict[str, int]
) -> None:
    """Paint pre-laid-out ``lines``, colouring each row's agent glyph.

    The renderers return plain strings — they have no business knowing about
    curses colour pairs — so the one piece of presentation left here is finding
    the glyph cell and repainting it. Everything else is a straight copy.
    """
    for i, line in enumerate(lines):
        y = top + i
        _put(stdscr, y, 0, line, width)
        found = find_glyph(line)
        if found is None:
            continue
        col, color_name = found
        attr = color_attrs.get(color_name)
        if attr:
            _put(stdscr, y, col, line[col], width, attr)


# ---------------------------------------------------------------------------
# Public entrypoint
# ---------------------------------------------------------------------------

def main(argv: list[str] | None = None) -> int:
    """Launch the petri TUI.

    Returns 0 when the user quits cleanly (``q``), 1 when the state file is
    missing (and prints the canonical error message to stderr in that case),
    and otherwise the exit code raised by ``curses.wrapper``.

    Never calls ``sys.exit`` itself; callers wrap with
    ``raise SystemExit(main())`` if they want the traditional behavior.
    """
    # Local var so the default is captured by the module-level name lookup at
    # *call* time, not at function-definition time — that keeps the value
    # monkey-patchable from tests without having to reimport the module.
    state_path = _DEFAULT_STATE_PATH

    if not os.path.isfile(state_path):
        print(
            f"no state file at {state_path}; run 'swab scan' first",
            file=sys.stderr,
        )
        return 1

    curses.wrapper(_run)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
