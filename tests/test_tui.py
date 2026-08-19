"""Tests for ``src/petridish/tui.py``.

Only the parts that do not need a real terminal are exercised here — anything
that goes through ``curses`` is skipped.  The missing-file path is tested
directly (it returns before `curses.wrapper` is ever invoked).  A small pure
helper, :func:`petridish.tui._format_stale_banner`, is also pinned so its
text contract can't silently drift.

Importantly: this module deliberately avoids spawning a curses window.  The
render loop itself needs a human to run ``petri`` and confirm it works in a
real terminal — those checks are manual, not scripted.
"""

from __future__ import annotations

import os
from datetime import datetime, timedelta, timezone

import pytest

import petridish.tui as tui
from petridish.schema import Radar


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _fake_radar(updated_at: datetime) -> Radar:
    """A minimal ``Radar`` carrying only ``updated_at`` — everything else
    defaults away, which keeps the helper tests decoupled from fixtures."""
    return Radar(updated_at=updated_at)


# ---------------------------------------------------------------------------
# Missing-state path (no terminal required)
# ---------------------------------------------------------------------------

def test_missing_state_returns_one_and_does_not_invoke_curses(tmp_path, capsys, monkeypatch):
    """``main()`` on a nonexistent state file:

    * returns 1,
    * prints the canonical error message verbatim to stderr,
    * never invokes ``curses.wrapper`` (because it bails before touching
      the terminal).

    The missing-file check reads `_DEFAULT_STATE_PATH` from the TUI module's
    namespace at call time — that is what we monkeypatch here.
    """
    fake_path = str(tmp_path / "no_such_state.json")

    # Point ``tui._DEFAULT_STATE_PATH`` (the module-level name that ``main``
    # resolves at *call* time) at a path nobody owns.
    monkeypatch.setattr("petridish.tui._DEFAULT_STATE_PATH", fake_path)

    # Guardrail: if ``main`` ever reaches the ``curses.wrapper`` line we want
    # to know immediately.
    never_called = {"n": 0}

    def _fake_wrapper(fn, *args, **kwargs):
        never_called["n"] += 1
        raise AssertionError("tui: curses.wrapper should not be called for missing-state")

    monkeypatch.setattr("curses.wrapper", _fake_wrapper)

    rc = tui.main()

    assert rc == 1
    err = capsys.readouterr().err
    expected = f"no state file at {fake_path}; run 'swab scan' first"
    assert expected in err, f"expected {expected!r} in stderr; got {err!r}"
    assert never_called["n"] == 0, "curses.wrapper was unexpectedly invoked"


# ---------------------------------------------------------------------------
# Pure helper — stale banner text
# ---------------------------------------------------------------------------

def test_format_stale_banner_uses_explicit_now():
    """``_format_stale_banner`` formats the age from an injected ``now``.

    Verifies the banner text contract so future edits can't silently change
    the wording or math.  A real curses render loop consumes this text —
    testing it here catches regression without needing a terminal.
    """
    # A radar whose last update was exactly 8h before ``now``.
    now = datetime(2026, 8, 6, 12, 0, 0, tzinfo=timezone.utc)
    radar = _fake_radar(now - timedelta(hours=8))

    banner = tui._format_stale_banner(radar, now=now)
    assert "data is 8.0h old" in banner
    assert "swab scan" in banner  # the actionable hint is preserved


def test_format_stale_banner_returns_short_string():
    """The banner is a short single-line caption — handy for regression if
    someone later grows it into a multi-line block."""
    now = datetime(2026, 8, 6, 12, 0, 0, tzinfo=timezone.utc)
    radar = _fake_radar(now - timedelta(hours=1))
    banner = tui._format_stale_banner(radar, now=now)
    assert "\n" not in banner, "banner must be a single line"
    assert len(banner) < 80


# ---------------------------------------------------------------------------
# Two-screen state machine.
#
# The pure key-handling helpers, so the screen toggle and the density toggle
# can be pinned without a terminal. The render loop itself still needs a human
# running ``petri`` — see the module docstring.
# ---------------------------------------------------------------------------

def test_next_screen_toggles_and_wraps():
    assert tui.next_screen("dashboard") == "browser"
    assert tui.next_screen("browser") == "dashboard"


def test_next_screen_round_trips():
    """tab twice must return you to where you started."""
    for start in ("dashboard", "browser"):
        assert tui.next_screen(tui.next_screen(start)) == start


def test_toggle_density_flips_what_is_currently_on_screen():
    """The first press must always change something visible.

    From the automatic default: 2 rows shows roomy, so z goes compact; 8 rows
    shows compact, so z goes roomy. A fixed cycle would make the first press a
    no-op whenever the automatic choice already matched the cycle's head.
    """
    assert tui.toggle_density(None, 2) == "compact"
    assert tui.toggle_density(None, 8) == "roomy"


def test_toggle_density_flips_an_existing_override_too():
    assert tui.toggle_density("roomy", 2) == "compact"
    assert tui.toggle_density("compact", 8) == "roomy"


def test_toggle_density_is_its_own_inverse():
    for n in (1, 4, 5, 20):
        first = tui.toggle_density(None, n)
        assert tui.toggle_density(first, n) != first


# ---------------------------------------------------------------------------
# Glyph colouring — the only presentation logic left in the blitter.
# ---------------------------------------------------------------------------

@pytest.mark.parametrize(
    ("line", "expected"),
    [
        (" ▲ rtk               feat/token-map", (1, "warn")),
        (" ● nono", (1, "green")),
        (" ○ old-blog", (1, "dim")),
        ("> ● project-radar", (2, "green")),
        ("  ▲ rtk", (2, "warn")),
    ],
)
def test_find_glyph_locates_the_state_cell(line, expected):
    assert tui.find_glyph(line) == expected


def test_find_glyph_returns_none_for_chrome_and_plain_rows():
    for line in (
        "petri · dashboard",
        "──────────────────",
        " RUNNING",
        "",
        "   petri-notes          main       commit 6d",
    ):
        assert tui.find_glyph(line) is None


def test_find_glyph_ignores_a_glyph_further_along_the_line():
    """A ● inside a commit message or branch name must not be repainted.

    Only the first few columns are searched, because that is where a row's own
    state glyph lives. Repainting a match mid-line would colour an arbitrary
    character green.
    """
    line = "   petri-notes    main    commit: fix ● bullet rendering"
    assert tui.find_glyph(line) is None


def test_find_glyph_ignores_the_browsers_right_hand_detail_pane():
    """Two-column rows carry a second half past the vertical rule.

    A glyph over there belongs to the detail text, not to the row's state, and
    it is far past column 4 — but pin it, because the right pane is the one
    place a second glyph can legitimately appear on the same line.
    """
    line = (
        "  ○ old-blog          main       0   -    "
        "│ agent state: idle ● was here"
    )
    assert tui.find_glyph(line) == (2, "dim")  # the row's own glyph, not the pane's
