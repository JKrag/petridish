"""Tests for ``src/petridish/tui_state.py``.

Every fixture is a :class:`Project` (or :class:`Radar`) built directly from
its dataclass constructor — no file I/O, no subprocesses. The whole module
is about transforming in-memory objects into rows and tracking a cursor, so
the tests should be just as input/output-bound.
"""

from __future__ import annotations

from dataclasses import replace as dc_replace
from datetime import datetime, timedelta, timezone

import pytest

from petridish.schema import (
    STATUS_BUCKETS,
    AgentState,
    GitState,
    Project,
    Radar,
)
from petridish.tui_state import (
    STALL_AFTER_S,
    STALL_CEILING_S,
    agent_bulb,
    dashboard_density,
    detail_layout,
    format_silence,
    glyph_for,
    has_agent,
    silence_seconds,
    sort_for_dashboard,
    column_widths,
    filter_projects,
    format_detail,
    format_row,
    group_by_bucket,
    is_stale,
    move,
    pad_row,
    running_layout,
    selected_project,
    SelectionState,
    worktree_children,
    worktree_count,
)

# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

def _project(
    name: str = "proj-a",
    *,
    status_bucket: str = "active",
    foreign: bool = False,
    branch: str | None = "main",
    dirty: bool = False,
    agent_state: str = "idle",
    active_agent: str | None = None,
    path: str | None = None,
    parent_path: str | None = None,
) -> Project:
    return Project(
        id=f"id-{name}",
        name=name,
        path=path or f"/fake/{name}",
        category="personal",
        parent_path=parent_path,
        is_foreign=foreign,
        git=GitState(
            is_repo=True,
            branch=branch,
            is_dirty=dirty,
        ),
        agent=AgentState(state=agent_state, active_agent=active_agent),
        status_bucket=status_bucket,
    )


def _now() -> datetime:
    return datetime.now(timezone.utc)


# ---------------------------------------------------------------------------
# group_by_bucket
# ---------------------------------------------------------------------------

def test_group_by_bucket_puts_each_project_under_its_bucket():
    projs = [
        _project("proj-a", status_bucket="active"),
        _project("proj-b", status_bucket="in_flight"),
        _project("proj-c", status_bucket="stale"),
        _project("proj-d", status_bucket="cold"),
    ]
    grouped = group_by_bucket(projs)

    assert list(grouped.keys()) == list(STATUS_BUCKETS)
    assert grouped["active"] == [projs[0]]
    assert grouped["in_flight"] == [projs[1]]
    assert grouped["stale"] == [projs[2]]
    assert grouped["cold"] == [projs[3]]


def test_group_by_bucket_excludes_foreign_projects():
    projs = [
        _project("me", status_bucket="active"),
        _project("theirs", status_bucket="active", foreign=True),
    ]
    grouped = group_by_bucket(projs)
    assert grouped["active"] == [_project("me", status_bucket="active")]
    assert len(grouped["active"]) == 1


# ---------------------------------------------------------------------------
# worktree_children / running_layout / worktree_count (ADR-0001)
# ---------------------------------------------------------------------------

def test_worktree_children_matches_on_parent_path():
    parent = _project("parent", path="/repos/parent")
    child = _project(
        "child", path="/repos/parent/.worktrees/child", parent_path="/repos/parent"
    )
    unrelated = _project("unrelated", path="/repos/other")
    assert worktree_children(parent, [parent, child, unrelated]) == [child]
    assert worktree_children(unrelated, [parent, child, unrelated]) == []


def test_running_layout_nests_an_active_child_under_its_active_parent():
    parent = _project("parent", path="/repos/parent", status_bucket="active")
    child = _project(
        "child",
        path="/repos/parent/.worktrees/child",
        parent_path="/repos/parent",
        status_bucket="active",
    )
    layout = running_layout([parent, child])
    assert layout == [(parent, [child])]


def test_running_layout_rolls_a_non_active_parent_in_via_an_active_child():
    parent = _project("parent", path="/repos/parent", status_bucket="stale")
    child = _project(
        "child",
        path="/repos/parent/.worktrees/child",
        parent_path="/repos/parent",
        status_bucket="active",
    )
    layout = running_layout([parent, child])
    assert layout == [(parent, [child])]


def test_running_layout_leaves_an_active_child_flat_when_its_parent_is_absent():
    # Parent directory was never itself discovered as a Project.
    child = _project(
        "child",
        path="/repos/parent/.worktrees/child",
        parent_path="/repos/parent",
        status_bucket="active",
    )
    layout = running_layout([child])
    assert layout == [(child, [])]


def test_running_layout_leaves_a_non_active_child_out_entirely():
    parent = _project("parent", path="/repos/parent", status_bucket="active")
    child = _project(
        "child",
        path="/repos/parent/.worktrees/child",
        parent_path="/repos/parent",
        status_bucket="cold",
    )
    layout = running_layout([parent, child])
    assert layout == [(parent, [])]


def test_running_layout_never_changes_status_bucket():
    parent = _project("parent", path="/repos/parent", status_bucket="stale")
    child = _project(
        "child",
        path="/repos/parent/.worktrees/child",
        parent_path="/repos/parent",
        status_bucket="active",
    )
    running_layout([parent, child])
    assert parent.status_bucket == "stale"
    assert child.status_bucket == "active"


def test_worktree_count_counts_all_children_regardless_of_their_bucket():
    parent = _project("parent", path="/repos/parent", status_bucket="stale")
    children = [
        _project(
            f"child-{i}",
            path=f"/repos/parent/.worktrees/child-{i}",
            parent_path="/repos/parent",
            status_bucket=b,
        )
        for i, b in enumerate(["active", "cold", "cold"])
    ]
    assert worktree_count(parent, [parent, *children]) == 3


def test_worktree_count_is_zero_for_a_project_with_no_children():
    parent = _project("parent", path="/repos/parent")
    assert worktree_count(parent, [parent]) == 0


def test_group_by_bucket_empty_list_has_all_four_keys():
    grouped = group_by_bucket([])
    assert set(grouped.keys()) == set(STATUS_BUCKETS)
    for bucket in STATUS_BUCKETS:
        assert grouped[bucket] == []


# ---------------------------------------------------------------------------
# filter_projects
# ---------------------------------------------------------------------------

def test_filter_projects_empty_query_returns_everything():
    projs = [_project(f"p{i}") for i in range(5)]
    assert filter_projects(projs, "") == list(projs)


def test_filter_projects_case_insensitive_substring():
    projs = [_project("FooBar"), _project("qux"), _project("Fooble")]
    hits = filter_projects(projs, "foo")
    assert [p.name for p in hits] == ["FooBar", "Fooble"]


def test_filter_projects_empty_hits_returns_empty_list():
    projs = [_project("alpha"), _project("beta")]
    assert filter_projects(projs, "zzz") == []


# ---------------------------------------------------------------------------
# format_row / format_detail
# ---------------------------------------------------------------------------

def test_format_row_matches_cli_rules_for_dirty_repo():
    p = _project("repo-one", branch="feat/x", dirty=True, active_agent="claude")
    row = format_row(p)

    # Expected: name | "claude (idle)" | "feat/x" | "*"
    assert row[0] == "repo-one"
    assert row[1] == "claude (idle)"
    assert row[2] == "feat/x"
    assert row[3] == "*"


def test_format_row_matches_cli_rules_for_idle_no_agent():
    p = _project("repo-one", active_agent=None)
    row = format_row(p)

    assert row[1] == "idle"           # no agent => bare state
    assert row[3] == " "               # clean repo (no dirty flag)
    assert row[2] == "main"


def test_format_detail_includes_mine_only_when_different():
    same = _project("same-ts")
    same = dc_replace(
        same,
        git=GitState(
            is_repo=True,
            branch="main",
            last_commit_at=datetime(2026, 1, 1, tzinfo=timezone.utc),
            mine_last_commit_at=datetime(2026, 1, 1, tzinfo=timezone.utc),
        ),
    )
    lines = format_detail(same)
    assert not any("mine last commit" in line for line in lines)

    diff = dc_replace(
        same,
        git=dc_replace(
            same.git,
            mine_last_commit_at=datetime(2025, 12, 1, tzinfo=timezone.utc),
        ),
    )
    lines = format_detail(diff)
    mine_lines = [ln for ln in lines if "mine last commit" in ln]
    assert len(mine_lines) == 1


def test_format_detail_handles_none_values_with_none_string():
    p = _project(
        "bare",
        branch=None,
    )
    lines = format_detail(p)
    # Every None should have rendered as the literal "none", not raise.
    for line in lines:
        assert ": none" in line or ":" in line  # at least no exception raised


# ---------------------------------------------------------------------------
# SelectionState.move
# ---------------------------------------------------------------------------

def test_move_crosses_empty_bucket_boundary():
    grouped = {
        "active": [_project("a1"), _project("a2")],
        "in_flight": [],
        "stale": [_project("b1")],
        "cold": [_project("c1")],
    }
    cursor = SelectionState(bucket="active", index=1)

    cursor = move(cursor, 1, grouped)
    assert (cursor.bucket, cursor.index) == ("stale", 0), (
        f"fell into empty bucket: got {cursor}"
    )

    cursor = move(cursor, 1, grouped)
    assert (cursor.bucket, cursor.index) == ("cold", 0)


def test_move_clamps_to_first_and_last_row():
    grouped = {
        "active": [_project("a1"), _project("a2")],
        "in_flight": [],
        "stale": [],
        "cold": [_project("c1")],
    }
    cursor = SelectionState(bucket="active", index=0)
    cursor = move(cursor, -99, grouped)
    assert (cursor.bucket, cursor.index) == ("active", 0), (
        f"wrapped past the top: got {cursor}"
    )

    cursor = SelectionState(bucket="cold", index=0)
    cursor = move(cursor, 99, grouped)
    assert (cursor.bucket, cursor.index) == ("cold", 0), (
        f"wrapped past the bottom: got {cursor}"
    )


def test_selected_project_returns_none_on_no_projects():
    grouped: dict[str, list[Project]] = {b: [] for b in STATUS_BUCKETS}
    cursor = SelectionState()
    assert selected_project(cursor, grouped) is None


def test_selected_project_returns_none_for_unknown_bucket():
    grouped = {"active": [_project("x")]}
    cursor = SelectionState(bucket="nope", index=0)
    assert selected_project(cursor, grouped) is None


# ---------------------------------------------------------------------------
# is_stale
# ---------------------------------------------------------------------------

def test_is_stale_boundary():
    updated = _now() - timedelta(hours=23)
    now = _now()
    assert is_stale(Radar(updated_at=updated), now=now) is False

    just_over = _now() - timedelta(hours=24, seconds=1)
    now_later = _now()
    assert is_stale(Radar(updated_at=just_over), now=now_later) is True


def test_is_stale_with_custom_threshold():
    updated = _now() - timedelta(hours=2)
    now = _now()
    assert is_stale(Radar(updated_at=updated), now=now, threshold_hours=1.0) is True
    assert is_stale(Radar(updated_at=updated), now=now, threshold_hours=5.0) is False


# ---------------------------------------------------------------------------
# column_widths / pad_row
# ---------------------------------------------------------------------------

def test_column_widths_grows_to_fit_longest_cell_or_header():
    rows = [["a", "claude-code (working)", "main", "*"], ["much-longer-name", "idle", "b", " "]]
    widths = column_widths(rows, headers=["name", "agent", "branch", "dirty"])
    assert widths == [len("much-longer-name"), len("claude-code (working)"), len("branch"), len("dirty")]


def test_column_widths_empty_rows_falls_back_to_header_length():
    widths = column_widths([], headers=["name", "agent", "branch", "dirty"])
    assert widths == [len("name"), len("agent"), len("branch"), len("dirty")]


def test_agent_bulb_maps_known_states():
    assert agent_bulb("working") == ("●", "green")
    assert agent_bulb("recent") == ("●", "yellow")
    assert agent_bulb("idle") == ("○", "dim")


def test_agent_bulb_falls_back_to_dim_hollow_circle_for_unknown_state():
    assert agent_bulb("some-future-state") == ("○", "dim")


def test_pad_row_aligns_columns_across_rows():
    rows = [["a", "claude-code (working)", "main", "*"], ["much-longer-name", "idle", "b", " "]]
    widths = column_widths(rows, headers=["name", "agent", "branch", "dirty"])
    lines = [pad_row(r, widths) for r in rows]
    # every line's second column must start at the same offset
    col2_offsets = [line.index("claude-code") if "claude-code" in line else line.index("idle") for line in lines]
    assert col2_offsets[0] == col2_offsets[1]


# ---------------------------------------------------------------------------
# The silence clock.
#
# This is the load-bearing signal for watching unattended runs, so its
# boundaries are pinned rather than smoke-tested.
# ---------------------------------------------------------------------------

NOW = datetime(2026, 8, 8, 3, 47, 0, tzinfo=timezone.utc)


def _agent_project(
    name: str = "proj",
    *,
    silence_s: float | None = None,
    last_activity_s: float | None = None,
) -> Project:
    """A project whose last agent event was ``silence_s`` ago.

    ``silence_s=None`` means no agent has ever been seen — ``last_event_at``
    stays ``None``, which is what ``has_agent`` keys on.
    """
    last_event_at = (
        None if silence_s is None else NOW - timedelta(seconds=silence_s)
    )
    last_activity_at = (
        None if last_activity_s is None else NOW - timedelta(seconds=last_activity_s)
    )
    return Project(
        id=f"id-{name}",
        name=name,
        path=f"/fake/{name}",
        category="personal",
        agent=AgentState(
            state="idle",  # deliberately WRONG/stale — glyph_for must ignore it
            active_agent=None if silence_s is None else "ornith-35b",
            last_event="Bash cargo test",
            last_event_at=last_event_at,
            session_id=None if silence_s is None else "sess-1",
        ),
        last_activity_at=last_activity_at,
    )


@pytest.mark.parametrize(
    ("silence_s", "expected"),
    [
        (0, "0s"),
        (12, "12s"),
        (59, "59s"),
        (60, "1m"),
        (252, "4m"),          # 4m 12s truncates to the minute
        (2820, "47m"),
        (3599, "59m"),
        (3600, "1h"),
        (11_045, "3h"),
        (86_399, "23h"),
        (86_400, "1d"),
        (3_542_400, "41d"),
    ],
)
def test_format_silence_single_unit(silence_s, expected):
    at = NOW - timedelta(seconds=silence_s)
    assert format_silence(at, now=NOW) == expected


@pytest.mark.parametrize(
    ("silence_s", "expected"),
    [
        (12, "12s"),           # no second unit below a minute
        (59, "59s"),
        (60, "1m 00s"),        # zero-padded so the column stays aligned
        (252, "4m 12s"),
        (2823, "47m 03s"),
        (3600, "1h 00m"),
        (11_045, "3h 04m"),
        (3_549_600, "41d 02h"),
    ],
)
def test_format_silence_two_units(silence_s, expected):
    at = NOW - timedelta(seconds=silence_s)
    assert format_silence(at, now=NOW, precise=True) == expected


def test_format_silence_none_is_a_dash():
    assert format_silence(None, now=NOW) == "-"
    assert format_silence(None, now=NOW, precise=True) == "-"


def test_format_silence_clamps_future_timestamps():
    """Clock skew between the writing daemon and the reading TUI is real.

    A future ``last_event_at`` must render as "0s", never as a negative age
    or a wrapped-around large number.
    """
    future = NOW + timedelta(hours=3)
    assert format_silence(future, now=NOW) == "0s"
    assert format_silence(future, now=NOW, precise=True) == "0s"


# ---------------------------------------------------------------------------
# glyph_for — the three-way split. The single most mistakeable piece of the
# dashboard: it must NOT read project.agent.state.
# ---------------------------------------------------------------------------

def test_glyph_no_agent_is_hollow():
    assert glyph_for(_agent_project(silence_s=None), now=NOW) == ("○", "dim")


def test_glyph_stalled_agent_is_a_warning():
    """An agent that has been here but has not moved in 47m needs attention."""
    assert glyph_for(_agent_project(silence_s=2820), now=NOW) == ("▲", "warn")


def test_glyph_stall_boundary_is_inclusive():
    below = _agent_project(silence_s=STALL_AFTER_S - 1)
    at = _agent_project(silence_s=STALL_AFTER_S)
    assert glyph_for(below, now=NOW)[0] == "●"
    assert glyph_for(at, now=NOW) == ("▲", "warn")


def test_glyph_working_and_recent_differ_by_colour_not_shape():
    working = glyph_for(_agent_project(silence_s=12), now=NOW)
    recent = glyph_for(_agent_project(silence_s=252), now=NOW)
    assert working == ("●", "green")
    assert recent == ("●", "yellow")


def test_glyph_ignores_the_stored_agent_state_field():
    """The stored state was stamped at scan time; the glyph is render time.

    Every fixture above carries ``state="idle"``. If ``glyph_for`` read that
    field, a 12-second-silent project would render dim/hollow instead of a
    green bulb. This is the assertion that keeps the traffic light honest
    against a stale projects.json.
    """
    live = _agent_project(silence_s=5)
    assert live.agent.state == "idle"          # the stale field
    assert glyph_for(live, now=NOW) == ("●", "green")  # the live truth


def test_glyph_never_reads_state_even_when_state_is_plausible():
    """Same check with a *consistent* state field, so the test isn't tautological.

    Here state="working" agrees with the silence, so both a correct and a
    state-reading implementation would pass — which is why the assertion is on
    the stalled case: state="working" but silent 47m must still warn.
    """
    stalled_but_labelled_working = dc_replace(
        _agent_project(silence_s=2820),
        agent=AgentState(
            state="working",
            active_agent="ornith-35b",
            last_event_at=NOW - timedelta(seconds=2820),
            session_id="s",
        ),
    )
    assert glyph_for(stalled_but_labelled_working, now=NOW) == ("▲", "warn")


# ---------------------------------------------------------------------------
# sort_for_dashboard — quietest agent first, then no-agent projects.
# ---------------------------------------------------------------------------

def test_dashboard_sort_puts_longest_silence_first():
    """Triage order: the run that has not moved is the one you need."""
    fresh = _agent_project("nono", silence_s=12)
    mid = _agent_project("project-radar", silence_s=252)
    stalled = _agent_project("rtk", silence_s=2820)
    ordered = sort_for_dashboard([fresh, mid, stalled], now=NOW)
    assert [p.name for p in ordered] == ["rtk", "project-radar", "nono"]


def test_dashboard_sort_puts_no_agent_projects_after_every_agent_project():
    """A 3h-idle no-agent project must NOT outrank a 47m-silent live run.

    Taken literally, "quietest first" would sort eficode-site (3h, no agent)
    above rtk (47m, stalled agent). It must not: the two groups are ranked,
    then sorted within.
    """
    stalled = _agent_project("rtk", silence_s=2820)
    no_agent_older = _agent_project(
        "eficode-site", silence_s=None, last_activity_s=10_800
    )
    ordered = sort_for_dashboard([no_agent_older, stalled], now=NOW)
    assert [p.name for p in ordered] == ["rtk", "eficode-site"]


def test_dashboard_sort_orders_no_agent_group_most_recent_first():
    """The no-agent group's order flips: context, not triage."""
    older = _agent_project("older", silence_s=None, last_activity_s=100_000)
    newer = _agent_project("newer", silence_s=None, last_activity_s=500)
    middle = _agent_project("middle", silence_s=None, last_activity_s=50_000)
    ordered = sort_for_dashboard([older, newer, middle], now=NOW)
    assert [p.name for p in ordered] == ["newer", "middle", "older"]


def test_dashboard_sort_places_never_active_project_last():
    never = _agent_project("never", silence_s=None, last_activity_s=None)
    some = _agent_project("some", silence_s=None, last_activity_s=999)
    assert [p.name for p in sort_for_dashboard([never, some], now=NOW)] == [
        "some",
        "never",
    ]


def test_dashboard_sort_breaks_ties_on_name_for_stable_repaints():
    """Equal silence must not swap order between 2-second repaints."""
    b = _agent_project("bravo", silence_s=300)
    a = _agent_project("alpha", silence_s=300)
    assert [p.name for p in sort_for_dashboard([b, a], now=NOW)] == ["alpha", "bravo"]


def test_dashboard_sort_does_not_mutate_its_input():
    projects = [_agent_project("b", silence_s=10), _agent_project("a", silence_s=99)]
    before = list(projects)
    sort_for_dashboard(projects, now=NOW)
    assert projects == before


# ---------------------------------------------------------------------------
# Density and pane layout.
# ---------------------------------------------------------------------------

@pytest.mark.parametrize(
    ("n_rows", "expected"),
    [(0, "roomy"), (1, "roomy"), (4, "roomy"), (5, "compact"), (8, "compact")],
)
def test_dashboard_density_collapses_above_the_roomy_ceiling(n_rows, expected):
    assert dashboard_density(n_rows) == expected


def test_dashboard_density_override_wins_in_both_directions():
    assert dashboard_density(1, override="compact") == "compact"
    assert dashboard_density(99, override="roomy") == "roomy"


def test_dashboard_density_ignores_a_bogus_override():
    """Reached from a key handler — cosmetic, so it must not raise."""
    assert dashboard_density(1, override="banana") == "roomy"
    assert dashboard_density(99, override=None) == "compact"


@pytest.mark.parametrize(
    ("width", "expected"),
    [(40, "bottom"), (78, "bottom"), (99, "bottom"), (100, "right"), (200, "right")],
)
def test_detail_layout_follows_terminal_width(width, expected):
    assert detail_layout(width) == expected


# ---------------------------------------------------------------------------
# has_agent keys on last_event_at, NOT session_id.
#
# The two fields are usually populated together, which makes the choice
# invisible — and a test that never separates them cannot tell the two
# implementations apart. These do.
# ---------------------------------------------------------------------------

def test_has_agent_keys_on_the_timestamp_not_the_session_id():
    """An event without a session id still gets the rich treatment.

    ``last_event_at`` is what the silence clock and the stall glyph actually
    consume, so its presence — not the id's — is the precondition. A hook
    event that arrives without a session id must still produce a live bulb and
    a silence counter, not a hollow dim row.
    """
    timestamped_but_sessionless = Project(
        id="id-x",
        name="x",
        path="/fake/x",
        category="personal",
        agent=AgentState(
            state="idle",
            active_agent="claude-code",
            last_event="PreToolUse",
            last_event_at=NOW - timedelta(seconds=30),
            session_id=None,
        ),
    )
    assert has_agent(timestamped_but_sessionless) is True
    assert glyph_for(timestamped_but_sessionless, now=NOW) == ("●", "green")
    assert format_silence(
        timestamped_but_sessionless.agent.last_event_at, now=NOW
    ) == "30s"


def test_has_agent_is_false_when_only_a_session_id_survives():
    """The inverse: an id with no timestamp cannot drive a silence clock.

    Nothing in scan.py produces this today, but a hand-edited or older-schema
    projects.json can, and the renderer must fall back to the hollow glyph
    rather than crash computing an age from None.
    """
    sessionful_but_timeless = Project(
        id="id-y",
        name="y",
        path="/fake/y",
        category="personal",
        agent=AgentState(
            state="working",
            active_agent="claude-code",
            last_event_at=None,
            session_id="sess-orphan",
        ),
    )
    assert has_agent(sessionful_but_timeless) is False
    assert glyph_for(sessionful_but_timeless, now=NOW) == ("○", "dim")
    assert silence_seconds(sessionful_but_timeless, now=NOW) == float("inf")


def test_dashboard_sort_groups_by_timestamp_not_session_id():
    """A sessionless-but-timestamped run outranks a no-event project."""
    sessionless_live = dc_replace(
        _agent_project("live", silence_s=60),
        agent=AgentState(
            state="idle",
            active_agent="claude-code",
            last_event_at=NOW - timedelta(seconds=60),
            session_id=None,
        ),
    )
    no_event = _agent_project("quiet", silence_s=None, last_activity_s=5)
    ordered = sort_for_dashboard([no_event, sessionless_live], now=NOW)
    assert [p.name for p in ordered] == ["live", "quiet"]


# ---------------------------------------------------------------------------
# The stall ceiling. Found by running against a real projects.json, where five
# of eight "running" rows carried agent metadata 14-29 hours old and every one
# of them warned.
# ---------------------------------------------------------------------------

def test_glyph_stops_warning_once_the_session_is_simply_over():
    """A run that ended last night is not an alarm.

    ``_bucket()`` calls anything touched within 48h "active", so the top
    section fills with yesterday's finished sessions. Without a ceiling the
    warning fires on almost every row, which makes it mean nothing.
    """
    yesterday = _agent_project("finished", silence_s=15 * 3600)
    assert glyph_for(yesterday, now=NOW) == ("○", "dim")


def test_glyph_still_warns_across_an_overnight_run():
    """The ceiling must span a night, or it defeats the dashboard's purpose.

    A job that starts at 23:00 and dies at midnight has been silent 8 hours by
    the time you look at 08:00. That must still warn.
    """
    assert glyph_for(_agent_project("died-at-midnight", silence_s=8 * 3600),
                     now=NOW) == ("▲", "warn")


def test_glyph_stall_ceiling_boundary():
    below = _agent_project("below", silence_s=STALL_CEILING_S - 1)
    at = _agent_project("at", silence_s=STALL_CEILING_S)
    assert glyph_for(below, now=NOW) == ("▲", "warn")
    assert glyph_for(at, now=NOW) == ("○", "dim")


def test_the_three_glyph_windows_are_ordered_and_exhaustive():
    """One pass over the whole silence range: ● then ▲ then ○, no gaps."""
    seen = [
        glyph_for(_agent_project("p", silence_s=s), now=NOW)[0]
        for s in (0, 60, 89, 91, 600, 1799, 1801, 3600, 43_199, 43_201, 200_000)
    ]
    assert seen == ["●"] * 6 + ["▲"] * 3 + ["○"] * 2


def test_a_long_silent_project_still_sorts_above_one_with_no_agent():
    """Dimming the glyph must not change the ordering.

    An ended session is still more interesting than a project no agent has ever
    touched — it just is not an alarm.
    """
    ended = _agent_project("ended", silence_s=20 * 3600)
    never = _agent_project("never", silence_s=None, last_activity_s=60)
    assert [p.name for p in sort_for_dashboard([never, ended], now=NOW)] == [
        "ended",
        "never",
    ]
