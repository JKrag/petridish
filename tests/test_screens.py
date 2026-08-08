"""Tests for ``src/petridish/screens.py`` — the two petri screen renderers.

Both renderers are pure ``Radar -> list[str]``, so every test here builds
dataclasses directly and asserts on strings. No terminal, no clock, no
filesystem: ``now`` and ``home`` are injected, which is the property that makes
these screens testable at all.

The exact-string snapshots are deliberate. Layout regressions (a lost rule, a
shifted column, a dropped section) are invisible to substring assertions and
obvious in a diff.
"""

from __future__ import annotations

import sys
from dataclasses import replace as dc_replace
from datetime import datetime, timedelta, timezone

import pytest

from petridish.schema import AgentState, GitState, Project, Radar
from petridish.screens import (
    DASHBOARD_KEYS,
    format_browser_row,
    format_card,
    format_detail_compact,
    render_browser,
    render_dashboard,
)
from petridish.tui_state import dashboard_density

NOW = datetime(2026, 8, 8, 3, 47, 0, tzinfo=timezone.utc)
HOME = "/Users/jankrag"


def _p(
    name: str,
    *,
    bucket: str = "active",
    silence_s: float | None = None,
    agent: str | None = None,
    event: str | None = None,
    session: str | None = None,
    branch: str | None = "main",
    dirty: int = 0,
    commit_h: float | None = None,
    github: bool = True,
    path: str | None = None,
    activity_s: float | None = None,
    foreign: bool = False,
) -> Project:
    return Project(
        id=f"id-{name}",
        name=name,
        path=path or f"{HOME}/repos/JKrag/{name}",
        category="JKrag",
        is_foreign=foreign,
        status_bucket=bucket,
        git=GitState(
            is_repo=True,
            branch=branch,
            is_dirty=dirty > 0,
            uncommitted_files=dirty,
            last_commit_at=None if commit_h is None else NOW - timedelta(hours=commit_h),
            mine_last_commit_at=(
                None if commit_h is None else NOW - timedelta(hours=commit_h)
            ),
            github_url=f"https://github.com/JKrag/{name}" if github else None,
        ),
        agent=AgentState(
            # Deliberately stale/wrong: nothing in these screens may read it.
            state="idle",
            active_agent=agent,
            last_event=event,
            last_event_at=(
                None if silence_s is None else NOW - timedelta(seconds=silence_s)
            ),
            session_id=session,
        ),
        last_activity_at=(
            None if activity_s is None else NOW - timedelta(seconds=activity_s)
        ),
    )


def _radar(*projects: Project, ms: int = 440) -> Radar:
    return Radar(updated_at=NOW, projects=projects, scan_duration_ms=ms)


# ---------------------------------------------------------------------------
# 1. Dashboard, roomy — the worked example, asserted as one exact string.
# ---------------------------------------------------------------------------

def test_dashboard_roomy_exact_string():
    radar = _radar(
        _p("rtk", silence_s=2820, agent="ornith-35b", branch="feat/token-map",
           dirty=3, event="Bash  cargo test --lib", session="2b81f004-aaaa-bbbb"),
        _p("nono", silence_s=12, agent="claude-opus-5", dirty=2,
           event="Bash  nono profile promote", session="7f2c1d90-1111"),
        _p("eficode-site", branch="feat/pricing", dirty=1, commit_h=3,
           activity_s=10800, path=f"{HOME}/repos/work/eficode-site"),
        _p("petri-notes", bucket="in_flight", commit_h=144, github=False),
        _p("old-blog", bucket="cold", commit_h=5800),
    )
    expected = "\n".join(
        [
            "petri · dashboard                                   5 projects · 03:47 · 0.44s",
            "══════════════════════════════════════════════════════════════════════════════",
            " RUNNING                                                    3 · quietest first",
            "──────────────────────────────────────────────────────────────────────────────",
            " ⚠ rtk                                                 silent 47m · ornith-35b",
            "     feat/token-map  ✎3                                 Bash  cargo test --lib",
            "     ~/repos/JKrag/rtk                                 sess 2b81f004-aaaa-bbbb",
            "",
            " ● nono                                             silent 12s · claude-opus-5",
            "     main  ✎2                                       Bash  nono profile promote",
            "     ~/repos/JKrag/nono                                     sess 7f2c1d90-1111",
            "",
            " ○ eficode-site                                                       no agent",
            "     feat/pricing  ✎1                                      commit 3h ago (you)",
            "     ~/repos/work/eficode-site",
            "",
            "──────────────────────────────────────────────────────────────────────────────",
            " IN FLIGHT                                                                   1",
            "──────────────────────────────────────────────────────────────────────────────",
            "   petri-notes          main                                   commit   6d   —",
            "",
            "──────────────────────────────────────────────────────────────────────────────",
            " STALE                                                              0 · COLD 1",
            "──────────────────────────────────────────────────────────────────────────────",
            "   old-blog",
            "──────────────────────────────────────────────────────────────────────────────",
            " tab browser   z density   q quit",
        ]
    )
    got = render_dashboard(radar, now=NOW, width=78, height=40, home=HOME)
    assert "\n".join(got) == expected


# ---------------------------------------------------------------------------
# 2. The ordering the whole screen exists for.
# ---------------------------------------------------------------------------

def test_dashboard_puts_the_stalled_run_on_the_first_card():
    """Longest-silence-first, visible in the rendered output.

    Constructed in the *opposite* order, so passing requires the renderer to
    actually sort rather than preserve insertion order.
    """
    radar = _radar(
        _p("fresh", silence_s=5, agent="a", session="s1"),
        _p("middling", silence_s=600, agent="a", session="s2"),
        _p("stalled", silence_s=4000, agent="a", session="s3"),
    )
    lines = render_dashboard(radar, now=NOW, width=78, height=40, home=HOME)
    names = [ln.split()[1] for ln in lines if ln.startswith((" ⚠ ", " ● ", " ○ "))]
    assert names == ["stalled", "middling", "fresh"]


def test_dashboard_marks_only_the_stalled_run_with_a_warning():
    radar = _radar(
        _p("fresh", silence_s=5, agent="a", session="s1"),
        _p("stalled", silence_s=4000, agent="a", session="s2"),
    )
    lines = render_dashboard(radar, now=NOW, width=78, height=40, home=HOME)
    warned = [ln for ln in lines if ln.startswith(" ⚠ ")]
    assert len(warned) == 1
    assert "stalled" in warned[0]


def test_dashboard_section_is_labelled_recent_when_nothing_has_an_agent():
    """"RUNNING" would be a lie over a section of idle-but-recent projects."""
    radar = _radar(_p("quiet", activity_s=3600, commit_h=1))
    lines = render_dashboard(radar, now=NOW, width=78, height=40, home=HOME)
    assert lines[2].startswith(" RECENT")
    assert not any(ln.startswith(" RUNNING") for ln in lines)


def test_dashboard_section_is_labelled_running_when_any_project_has_an_agent():
    radar = _radar(
        _p("quiet", activity_s=3600, commit_h=1),
        _p("live", silence_s=10, agent="a", session="s"),
    )
    lines = render_dashboard(radar, now=NOW, width=78, height=40, home=HOME)
    assert lines[2].startswith(" RUNNING")


# ---------------------------------------------------------------------------
# 3. Dashboard, compact — the eight-run night.
# ---------------------------------------------------------------------------

def _eight_runs() -> Radar:
    spec = [
        ("rtk", 2820, "feat/token-map", 3, "Bash  cargo test --lib"),
        ("swab-hook-bench", 2280, "wip/latency", 3, "Read  bench/results.md"),
        ("mlx-playground", 1320, "main", 1, "Bash  uv run pytest -q"),
        ("petri-notes", 540, "main", 0, "Write docs/outline.md"),
        ("project-radar", 252, "master", 7, "Edit  src/petridish/tui.py"),
        ("delegate-to-local", 120, "feat/rounds", 12, "Bash  git diff --stat"),
        ("nono", 12, "main", 2, "Bash  nono profile promote"),
        ("kata-2025", 4, "main", 0, "Edit  src/day07.py"),
    ]
    return _radar(
        *[
            _p(n, silence_s=s, agent="ornith-35b", branch=b, dirty=d, event=e,
               session="s")
            for n, s, b, d, e in spec
        ],
        ms=510,
    )


def test_dashboard_compact_exact_string():
    expected = "\n".join(
        [
            "petri · dashboard                                   8 projects · 03:47 · 0.51s",
            "══════════════════════════════════════════════════════════════════════════════",
            " RUNNING                                          8 · quietest first · compact",
            "──────────────────────────────────────────────────────────────────────────────",
            " ⚠ rtk               feat/token-map   ✎3    47m  Bash  cargo test --lib",
            " ⚠ swab-hook-bench   wip/latency      ✎3    38m  Read  bench/results.md",
            " ● mlx-playground    main             ✎1    22m  Bash  uv run pytest -q",
            " ● petri-notes       main                    9m  Write docs/outline.md",
            " ● project-radar     master           ✎7     4m  Edit  src/petridish/tui.py",
            " ● delegate-to-local feat/rounds     ✎12     2m  Bash  git diff --stat",
            " ● nono              main             ✎2    12s  Bash  nono profile promote",
            " ● kata-2025         main                    4s  Edit  src/day07.py",
            "",
            "──────────────────────────────────────────────────────────────────────────────",
            " tab browser   z density   q quit",
        ]
    )
    got = render_dashboard(
        _eight_runs(), now=NOW, width=78, height=40, density="compact", home=HOME
    )
    assert "\n".join(got) == expected


def test_compact_density_is_what_eight_runs_selects():
    """The renderer and the density picker must agree on the eight-run case."""
    assert dashboard_density(8) == "compact"
    assert dashboard_density(4) == "roomy"


def test_compact_fits_eight_runs_where_roomy_would_not():
    """The reason two densities exist, asserted as what survives the budget.

    Eight roomy cards are 32 lines. On a 40-row terminal that leaves room for
    the in-flight section and nothing after it: the stale projects and the
    keymap are both pushed off the screen. Compact spends 8 lines on the same
    eight runs and everything fits with 16 rows to spare.
    """
    radar = _radar(
        *_eight_runs().projects,
        _p("wip", bucket="in_flight", commit_h=96),
        _p("dusty", bucket="stale", commit_h=1000),
        ms=510,
    )
    roomy = render_dashboard(radar, now=NOW, width=78, height=40, density="roomy")
    compact = render_dashboard(radar, now=NOW, width=78, height=40, density="compact")

    # Roomy: cards exhaust the budget, so the tail of the screen is lost.
    assert len(roomy) == 40
    assert not any("STALE" in ln for ln in roomy)
    assert not any("dusty" in ln for ln in roomy)
    assert not any(DASHBOARD_KEYS in ln for ln in roomy)

    # Compact: everything fits, with rows to spare.
    assert len(compact) == 24
    assert any("STALE" in ln for ln in compact)
    assert any("dusty" in ln for ln in compact)
    assert any(DASHBOARD_KEYS in ln for ln in compact)


# ---------------------------------------------------------------------------
# 4. Budget invariants — the properties tui.py relies on to be a dumb blitter.
# ---------------------------------------------------------------------------

@pytest.mark.parametrize("width", [40, 60, 78, 100, 140])
@pytest.mark.parametrize("height", [5, 12, 24, 40])
@pytest.mark.parametrize("density", ["roomy", "compact"])
def test_dashboard_never_exceeds_its_budget(width, height, density):
    lines = render_dashboard(
        _eight_runs(), now=NOW, width=width, height=height, density=density, home=HOME
    )
    assert len(lines) <= height
    assert all(len(ln) <= width for ln in lines), [
        ln for ln in lines if len(ln) > width
    ]


@pytest.mark.parametrize("width", [40, 60, 78, 99, 100, 140])
@pytest.mark.parametrize("height", [5, 12, 24, 40])
def test_browser_never_exceeds_its_budget(width, height):
    radar = _eight_runs()
    lines = render_browser(
        radar,
        now=NOW,
        width=width,
        height=height,
        selected=radar.projects[0],
        home=HOME,
    )
    assert len(lines) <= height
    assert all(len(ln) <= width for ln in lines), [
        ln for ln in lines if len(ln) > width
    ]


def test_dashboard_drops_low_priority_sections_first_on_a_short_terminal():
    """A short terminal must lose the cold projects, not the stalled run."""
    radar = _radar(
        _p("stalled", silence_s=4000, agent="a", session="s"),
        _p("old", bucket="cold", commit_h=9000),
    )
    lines = render_dashboard(radar, now=NOW, width=78, height=6, home=HOME)
    assert any("stalled" in ln for ln in lines)
    assert not any("old" in ln for ln in lines)


# ---------------------------------------------------------------------------
# 5. The browser's two layouts.
# ---------------------------------------------------------------------------

def test_browser_uses_a_right_pane_at_100_columns():
    radar = _eight_runs()
    lines = render_browser(
        radar, now=NOW, width=100, height=24, selected=radar.projects[0], home=HOME
    )
    assert "╤" in lines[1]
    assert any("│" in ln for ln in lines)


def test_browser_uses_a_bottom_pane_below_100_columns():
    radar = _eight_runs()
    lines = render_browser(
        radar, now=NOW, width=99, height=24, selected=radar.projects[0], home=HOME
    )
    assert "╤" not in lines[1]
    assert not any("│" in ln for ln in lines)


def test_browser_layout_can_be_forced_against_the_width():
    """The width rule is a default, not a cage — ``swab dash`` may override."""
    radar = _eight_runs()
    forced = render_browser(
        radar, now=NOW, width=140, height=24, layout="bottom",
        selected=radar.projects[0], home=HOME,
    )
    assert not any("│" in ln for ln in forced)


def test_browser_right_pane_keeps_the_silence_column_and_drops_agent():
    """The regression this budgeting exists for.

    Blind truncation at width 100 left ~52 columns for the table and the
    ellipsis ate ``silent`` — the one value the screen exists to show. Dropping
    the whole ``agent`` column (already named in the detail pane) keeps it.
    """
    radar = _eight_runs()
    lines = render_browser(
        radar, now=NOW, width=100, height=24, selected=radar.projects[0], home=HOME
    )
    header = next(ln for ln in lines if "name" in ln and "silent" in ln)
    assert "silent" in header
    assert "agent" not in header
    # And the values are really there, not ellipsised away. Search left of the
    # vertical rule: the detail pane on the right also spells out the name.
    rtk = next(ln for ln in lines if "rtk" in ln.split("│")[0])
    assert "47m" in rtk


def test_browser_bottom_pane_keeps_every_column_when_width_allows():
    radar = _eight_runs()
    lines = render_browser(
        radar, now=NOW, width=78, height=24, selected=radar.projects[0], home=HOME
    )
    header = next(ln for ln in lines if "name" in ln and "silent" in ln)
    assert "agent" in header
    assert "silent" in header


def test_browser_marks_the_selected_row_and_only_that_row():
    radar = _eight_runs()
    lines = render_browser(
        radar, now=NOW, width=78, height=40, selected=radar.projects[2], home=HOME
    )
    marked = [ln for ln in lines if ln.startswith(">")]
    assert len(marked) == 1
    assert radar.projects[2].name in marked[0]


def test_browser_with_no_selection_marks_nothing():
    lines = render_browser(_eight_runs(), now=NOW, width=78, height=40, home=HOME)
    assert not any(ln.startswith(">") for ln in lines)


def test_browser_query_filters_the_list():
    radar = _eight_runs()
    lines = render_browser(radar, now=NOW, width=78, height=40, query="nono", home=HOME)
    assert any("nono" in ln for ln in lines)
    assert not any("kata-2025" in ln for ln in lines)


def test_browser_reports_an_empty_result_rather_than_a_blank_screen():
    lines = render_browser(
        _eight_runs(), now=NOW, width=78, height=40, query="zzz-nothing", home=HOME
    )
    assert any("no projects match" in ln for ln in lines)


# ---------------------------------------------------------------------------
# 6. Foreign projects, so the two screens agree on how many projects exist.
# ---------------------------------------------------------------------------

def test_neither_screen_shows_foreign_projects():
    radar = _radar(
        _p("mine", silence_s=10, agent="a", session="s"),
        _p("theirs", silence_s=10, agent="a", session="s", foreign=True),
    )
    dash = render_dashboard(radar, now=NOW, width=78, height=40, home=HOME)
    browse = render_browser(radar, now=NOW, width=78, height=40, home=HOME)
    for lines in (dash, browse):
        assert any("mine" in ln for ln in lines)
        assert not any("theirs" in ln for ln in lines)


def test_dashboard_running_count_excludes_foreign_projects():
    """A count that disagreed with the rows below it would be worse than none."""
    radar = _radar(
        _p("mine", silence_s=10, agent="a", session="s"),
        _p("theirs", silence_s=10, agent="a", session="s", foreign=True),
    )
    lines = render_dashboard(radar, now=NOW, width=78, height=40, home=HOME)
    assert lines[2].rstrip().endswith("1 · quietest first")


# ---------------------------------------------------------------------------
# 7. Cards and rows in isolation.
# ---------------------------------------------------------------------------

def test_card_for_a_no_agent_project_spends_its_lines_on_git():
    card = format_card(
        _p("site", branch="feat/x", dirty=1, commit_h=3), now=NOW, width=78, home=HOME
    )
    assert card[0].endswith("no agent")
    assert "commit 3h ago (you)" in card[1]
    assert "sess" not in "\n".join(card)


def test_card_omits_the_you_marker_when_the_last_commit_is_not_yours():
    project = dc_replace(
        _p("shared"),
        git=GitState(
            is_repo=True,
            branch="main",
            last_commit_at=NOW - timedelta(hours=3),
            # Someone else's commit: the two timestamps differ.
            mine_last_commit_at=NOW - timedelta(hours=99),
        ),
    )
    card = format_card(project, now=NOW, width=78, home=HOME)
    assert "commit 3h ago" in card[1]
    assert "(you)" not in card[1]


def test_card_collapses_the_home_directory():
    card = format_card(
        _p("x", silence_s=5, agent="a", session="s"), now=NOW, width=78, home=HOME
    )
    assert "~/repos/JKrag/x" in card[2]
    assert HOME not in card[2]


def test_card_leaves_the_path_alone_without_a_home():
    card = format_card(_p("x", silence_s=5, agent="a", session="s"), now=NOW, width=78)
    assert f"{HOME}/repos/JKrag/x" in card[2]


def test_card_has_a_trailing_blank_separator():
    card = format_card(_p("x", silence_s=5, agent="a", session="s"), now=NOW, width=78)
    assert len(card) == 4
    assert card[-1] == ""


def test_browser_row_shows_a_dash_for_silence_without_an_agent():
    row = format_browser_row(_p("x", commit_h=1), now=NOW)
    assert row[-1] == "-"


def test_browser_row_shows_zero_rather_than_blank_for_a_clean_repo():
    """A blank dirty cell reads as missing data; 0 reads as clean."""
    assert format_browser_row(_p("x", dirty=0), now=NOW)[3] == "0"
    assert format_browser_row(_p("x", dirty=7), now=NOW)[3] == "7"


def test_in_flight_rows_carry_no_agent_column():
    """Safe by construction: _bucket() returns "active" for any live agent.

    So an in_flight project can never have a running agent, and the width that
    would carry agent state is spent on commit age instead.
    """
    radar = _radar(_p("wip", bucket="in_flight", branch="feat/x", dirty=2, commit_h=96))
    lines = render_dashboard(radar, now=NOW, width=78, height=40, home=HOME)
    row = next(ln for ln in lines if "wip" in ln and "commit" in ln)
    assert "commit   4d" in row
    assert "gh" in row
    assert "silent" not in row


def test_detail_compact_is_three_lines_and_carries_what_a_row_cannot():
    project = _p(
        "project-radar", silence_s=252, agent="ornith-35b", branch="master", dirty=7,
        event="Edit  src/petridish/tui.py", session="a43ebbd0-d1b2-4c65", commit_h=1,
    )
    lines = format_detail_compact(project, now=NOW, width=78, home=HOME)
    assert len(lines) == 3
    assert "silent 4m 12s" in lines[0]  # precise form, unlike the list column
    assert "~/repos/JKrag/project-radar" in lines[1]
    assert "sess a43ebbd0-d1b2-4c65" in lines[1]
    assert "Edit  src/petridish/tui.py" in lines[2]


def test_detail_compact_handles_a_project_with_no_agent():
    lines = format_detail_compact(_p("x", commit_h=2), now=NOW, width=78, home=HOME)
    assert len(lines) == 3
    assert "no agent" in lines[0]
    assert "sess" not in lines[1]


# ---------------------------------------------------------------------------
# 8. Degenerate inputs. Neither screen may raise.
# ---------------------------------------------------------------------------

def test_empty_radar_renders_both_screens():
    empty = Radar(updated_at=NOW)
    dash = render_dashboard(empty, now=NOW, width=78, height=40, home=HOME)
    browse = render_browser(empty, now=NOW, width=78, height=40, home=HOME)
    assert any("nothing active" in ln for ln in dash)
    assert any("no projects match" in ln for ln in browse)
    assert all(len(ln) <= 78 for ln in dash + browse)


def test_absurdly_narrow_terminal_does_not_raise():
    radar = _eight_runs()
    for width in (1, 2, 5, 10):
        for fn in (render_dashboard, render_browser):
            lines = fn(radar, now=NOW, width=width, height=10, home=HOME)
            assert all(len(ln) <= width for ln in lines)


def test_zero_height_returns_nothing():
    radar = _eight_runs()
    assert render_dashboard(radar, now=NOW, width=78, height=0) == []
    assert render_browser(radar, now=NOW, width=78, height=0) == []


def test_future_timestamps_do_not_break_the_dashboard():
    """Clock skew between the writing daemon and the reading TUI."""
    future = _p("skewed", silence_s=-3600, agent="a", session="s")
    lines = render_dashboard(_radar(future), now=NOW, width=78, height=40, home=HOME)
    assert any("silent 0s" in ln for ln in lines)


def test_missing_branch_renders_a_dash_not_none():
    radar = _radar(_p("nogit", branch=None, silence_s=5, agent="a", session="s"))
    lines = render_dashboard(radar, now=NOW, width=78, height=40, home=HOME)
    assert not any("None" in ln for ln in lines)
    assert any(ln.strip().startswith("-") for ln in lines)


if __name__ == "__main__":  # pragma: no cover
    sys.exit(__import__("pytest").main([__file__, "-v"]))


# ---------------------------------------------------------------------------
# 9. Gaps found by mutation testing. Each of these mutations initially
#    SURVIVED the suite above, which means the suite was not checking them.
# ---------------------------------------------------------------------------

def test_dirty_marker_requires_both_the_flag_and_a_nonzero_count():
    """``is_dirty`` and ``uncommitted_files`` must be checked independently.

    Every other fixture here derives one from the other, so a renderer that
    ignored the count passed anyway. This is the same contract ``menubar.py``
    enforces — both frontends show ``✎N`` only when both hold.
    """
    dirty_but_empty = Project(
        id="id-d", name="d", path=f"{HOME}/d", category="x",
        git=GitState(is_repo=True, branch="main", is_dirty=True, uncommitted_files=0),
        agent=AgentState(last_event_at=NOW - timedelta(seconds=5), session_id="s"),
    )
    card = format_card(dirty_but_empty, now=NOW, width=78, home=HOME)
    assert "✎" not in "\n".join(card)
    assert format_browser_row(dirty_but_empty, now=NOW)[3] == "0"


def test_header_project_count_excludes_foreign_projects():
    """The count must match the rows beneath it.

    ``group_by_bucket`` drops foreign projects from the list, so a header built
    from ``len(radar.projects)`` claimed 2 projects above a list of 1.
    """
    radar = _radar(
        _p("mine", silence_s=10, agent="a", session="s"),
        _p("theirs", silence_s=10, agent="a", session="s", foreign=True),
    )
    for lines in (
        render_dashboard(radar, now=NOW, width=78, height=40, home=HOME),
        render_browser(radar, now=NOW, width=78, height=40, home=HOME),
        render_browser(radar, now=NOW, width=120, height=40, home=HOME),
    ):
        assert "1 projects" in lines[0], lines[0]
        assert "2 projects" not in lines[0]


def test_browser_bottom_pane_reserves_its_rows_from_the_list():
    """The bug the old render loop had: pane and list fighting for the same rows.

    With no reservation the list consumes the full height, and the detail pane
    plus keymap are appended past the budget — where the clip silently drops
    them.

    This only bites when the list is *longer* than the available rows, which is
    why the fixture is 24 projects in a 12-row terminal. An eight-project radar
    fits inside the budget with or without the reservation, so it cannot tell
    the two apart.
    """
    radar = _radar(
        *[
            _p(f"proj-{i:02d}", silence_s=10 * i, agent="a", session=f"s{i}",
               event="Bash  x")
            for i in range(24)
        ]
    )
    lines = render_browser(
        radar, now=NOW, width=78, height=12, selected=radar.projects[0], home=HOME
    )
    assert len(lines) <= 12
    assert any("sess" in ln for ln in lines), "detail pane was pushed off-screen"
    assert any("tab dashboard" in ln for ln in lines), "keymap was pushed off-screen"


def test_in_flight_rows_are_sorted_by_name():
    """Deterministic order; scan order is filesystem order and varies."""
    radar = _radar(
        _p("zulu", bucket="in_flight", commit_h=96),
        _p("alpha", bucket="in_flight", commit_h=96),
        _p("mike", bucket="in_flight", commit_h=96),
    )
    lines = render_dashboard(radar, now=NOW, width=78, height=40, home=HOME)
    rows = [ln for ln in lines if "commit" in ln and "IN FLIGHT" not in ln]
    names = [ln.split()[0] for ln in rows]
    assert names == ["alpha", "mike", "zulu"]
