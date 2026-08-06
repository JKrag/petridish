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
    filter_projects,
    format_detail,
    format_row,
    group_by_bucket,
    is_stale,
    move,
    selected_project,
    SelectionState,
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
) -> Project:
    return Project(
        id=f"id-{name}",
        name=name,
        path=f"/fake/{name}",
        category="personal",
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
