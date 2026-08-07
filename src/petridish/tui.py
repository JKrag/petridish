"""Interactive TUI for the petridish project radar.

Renders :class:`~petridish.schema.Radar` as a bucketed table inside a curses
terminal, with selection, search, refresh-on-disk-change, and a detail panel.

State management — grouping, filtering, row formatting, cursor movement — is
delegated to :mod:`petridish.tui_state`.  This module owns only the curses
render loop and the process entrypoint.

The entry point :func:`main` never calls ``sys.exit`` itself; tests call it
directly and assert on the returned exit code.
"""

from __future__ import annotations

import curses
import os
import sys
from datetime import datetime, timezone

from petridish.cli import _DEFAULT_STATE_PATH
from petridish.schema import read_json
from petridish.tui_state import (
    ROW_HEADERS,
    SelectionState,
    agent_bulb,
    column_widths,
    filter_projects,
    format_detail,
    format_row,
    group_by_bucket,
    is_stale,
    move,
    pad_row,
    selected_project,
)

#: Width reserved at the start of every project row for the agent-state
#: bulb (glyph + one space). Column headers and rows both shift right by
#: this much so "name" lines up under the first real name column.
_BULB_WIDTH = 2

#: How often getch() gives up and returns -1, in milliseconds. This is what
#: makes the mtime-poll auto-refresh actually run without a keypress — a
#: blocking getch() (the default) never returns, so the reload check at the
#: top of the loop only ran when the user happened to press a key.
_POLL_MS = 2000


def _init_color_attrs() -> dict[str, int]:
    """Best-effort color pairs for the agent-state bulb.

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
        return {
            "green": curses.color_pair(1) | curses.A_BOLD,
            "yellow": curses.color_pair(2) | curses.A_BOLD,
            "dim": curses.color_pair(3) | curses.A_DIM,
        }
    except curses.error:
        return {}


# ---------------------------------------------------------------------------
# Pure helpers (no curses, testable without a terminal)
# ---------------------------------------------------------------------------

def _format_stale_banner(radar, *, now: datetime | None = None) -> str:
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


# ---------------------------------------------------------------------------
# Curses render loop
# ---------------------------------------------------------------------------

def _run(stdscr) -> int:
    """The curses render loop. Called inside ``curses.wrapper``."""
    curses.curs_set(0)  # hide the text cursor
    stdscr.timeout(_POLL_MS)  # getch() returns -1 after this many ms of no input
    color_attrs = _init_color_attrs()

    state_path = _DEFAULT_STATE_PATH
    radar = read_json(state_path)
    last_mtime = os.stat(state_path).st_mtime

    query: str = ""
    state = SelectionState()
    search_mode = False
    search_buffer: str = ""

    # Compute flat-then-grouped views up front.
    filtered = filter_projects(list(radar.projects), query)
    grouped = group_by_bucket(filtered)

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
                filtered = filter_projects(list(radar.projects), query)
                grouped = group_by_bucket(filtered)
                state = SelectionState()
                last_mtime = current_mtime

            # ---- render one frame ----
            stdscr.erase()
            h, w = stdscr.getmaxyx()

            # Compute the detail panel's content *before* drawing the list,
            # so its lines (plus one separator) can be reserved at the
            # bottom of the screen rather than fighting the list for
            # whatever's left over.
            selected = selected_project(state, grouped)
            detail_lines = format_detail(selected) if selected is not None else []
            reserved = (1 + len(detail_lines)) if detail_lines else 0

            # Stale banner at row 0.
            banner_line = ""
            if is_stale(radar, now=datetime.now(timezone.utc)):
                banner_line = _format_stale_banner(radar)
                try:
                    stdscr.addnstr(0, 0, banner_line, w - 1, curses.A_BOLD)
                except curses.error:
                    pass

            top_y = 1 if banner_line else 0
            list_bottom = max(top_y, h - reserved)
            cursor_y = top_y

            # Column widths computed once, across every visible row, so
            # columns line up across bucket sections the way swab list's do.
            all_rows = [
                format_row(p) for projects in grouped.values() for p in projects
            ]
            widths = column_widths(all_rows, ROW_HEADERS)

            if cursor_y < list_bottom:
                header_line = pad_row(ROW_HEADERS, widths)
                try:
                    stdscr.addnstr(cursor_y, _BULB_WIDTH, header_line.ljust(w - 1), w - 1,
                                   curses.A_UNDERLINE)
                except curses.error:
                    pass
                cursor_y += 1

            # Bucket sections: header + aligned rows.
            for bucket_name, projects in grouped.items():
                if cursor_y >= list_bottom:
                    break

                header = f"[{bucket_name.title()}] ({len(projects)})"
                try:
                    stdscr.addnstr(cursor_y, 0, header.ljust(w - 1), w - 1,
                                   curses.A_BOLD)
                except curses.error:
                    pass
                cursor_y += 1

                for i, proj in enumerate(projects):
                    if cursor_y >= list_bottom:
                        break
                    row_str = pad_row(format_row(proj), widths).ljust(w - 1)
                    selected_attr = (
                        curses.A_REVERSE
                        if state.bucket == bucket_name and state.index == i
                        else 0
                    )
                    glyph, color_name = agent_bulb(proj.agent.state)
                    bulb_attr = color_attrs.get(color_name, 0) | selected_attr
                    try:
                        stdscr.addnstr(cursor_y, 0, glyph, 1, bulb_attr)
                        stdscr.addnstr(cursor_y, _BULB_WIDTH, row_str,
                                       max(0, w - 1 - _BULB_WIDTH), selected_attr)
                    except curses.error:
                        pass
                    cursor_y += 1

            # Separator + detail panel in the region reserved above.
            if detail_lines:
                sep_y = h - reserved
                try:
                    stdscr.addnstr(sep_y, 0, "-" * max(0, w - 1), w - 1, curses.A_DIM)
                except curses.error:
                    pass
                for di, dline in enumerate(detail_lines):
                    dy = sep_y + 1 + di
                    if dy < 0 or dy >= h:
                        continue
                    try:
                        stdscr.addnstr(dy, 0, dline.ljust(w - 1), w - 1)
                    except curses.error:
                        pass

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
                filtered = filter_projects(list(radar.projects), query)
                grouped = group_by_bucket(filtered)
                state = SelectionState()
            elif ch == 27:  # Esc
                search_mode = False
                search_buffer = ""
            elif ch in (curses.KEY_BACKSPACE, 127, 8):
                search_buffer = search_buffer[:-1] if search_buffer else ""
            elif 32 <= ch < 127:
                search_buffer += chr(ch)
            continue

        if ch == -1:
            continue
        if ch in (ord('q'), ord('Q')):
            return 0
        elif ch in (curses.KEY_UP, ord('k')):
            state = move(state, -1, grouped)
        elif ch in (curses.KEY_DOWN, ord('j')):
            state = move(state, 1, grouped)
        elif ch == ord('/'):
            search_mode = True
            search_buffer = ""

    return 0


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
