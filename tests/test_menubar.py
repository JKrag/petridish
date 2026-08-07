"""Tests for ``src/petridish/menubar.py`` — the menubar text renderer."""

from __future__ import annotations

import sys
from datetime import datetime, timezone

from petridish.menubar import render_menubar
from petridish.schema import AgentState, GitState, Project, Radar

# ---------------------------------------------------------------------------
# 1. The worked example from the contract.
# ---------------------------------------------------------------------------

def test_worked_example_matches_exact_string():
    """The literal worked example in the spec, asserted as one exact string.

    Keeping this as a single equality — rather than line-by-line — makes any
    regression in header/footer/format immediately obvious.
    """
    alpha = Project(
        name="alpha", path="/p/alpha", category="demo", id="id-alpha",
        status_bucket="active",
        git=GitState(is_repo=True, branch="main", is_dirty=True, uncommitted_files=3),
        agent=AgentState(state="working"),
    )
    beta = Project(
        name="beta", path="/p/beta", category="demo", id="id-beta",
        status_bucket="cold",
        git=GitState(is_repo=False),
        agent=AgentState(state="idle"),
    )
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
        projects=(alpha, beta),
    )

    expected = (
        "🧫 1/2\n"
        "---\n"
        "Active\n"
        "--alpha · main ✎3 ● | href=file:///p/alpha\n"
        "Cold\n"
        "--beta | href=file:///p/beta\n"
        "---\n"
        "Refresh | refresh=true"
    )

    assert render_menubar(radar) == expected


# ---------------------------------------------------------------------------
# 2. Empty radar.
# ---------------------------------------------------------------------------

def test_empty_radar_exact_string():
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
    )
    expected = (
        "🧫 0/0\n"
        "---\n"
        "No projects | color=#888888\n"
        "---\n"
        "Refresh | refresh=true"
    )

    assert render_menubar(radar) == expected


# ---------------------------------------------------------------------------
# 3. Bucket ordering: Active must come before Cold regardless of construction
#    order (Radar.projects preserves insertion order).
# ---------------------------------------------------------------------------

def test_bucket_ordering_active_before_cold():
    a_active = Project(id="t-0", category="demo", 
        name="zebra-active", path="/p/za", status_bucket="active",
        agent=AgentState(state="idle"),
    )
    b_cold = Project(id="t-1", category="demo", 
        name="alpha-cold", path="/p/ac", status_bucket="cold",
        agent=AgentState(state="idle"),
    )
    # Construction order: cold first, active second — but Active must still
    # precede Cold in the output.
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
        projects=(b_cold, a_active),
    )

    out = render_menubar(radar)
    # ``Active`` is one label; finding it at a lower index than ``Cold`` proves
    # the bucket order is honoured regardless of input order.
    active_idx = out.index("Active")
    cold_idx = out.index("Cold")
    assert active_idx < cold_idx


# ---------------------------------------------------------------------------
# 4. Projects within a bucket are sorted by name.
# ---------------------------------------------------------------------------

def test_projects_within_bucket_sorted_by_name():
    a = Project(id="t-2", category="demo", name="cherry", path="/p/c", status_bucket="active",
                agent=AgentState(state="idle"))
    b = Project(id="t-3", category="demo", name="apple",  path="/p/a", status_bucket="active",
                agent=AgentState(state="idle"))
    c = Project(id="t-4", category="demo", name="banana", path="/p/b", status_bucket="active",
                agent=AgentState(state="idle"))
    # Construction in reverse alphabetical order — output must still be A→C.
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
        projects=(c, a, b),
    )
    lines = render_menubar(radar).splitlines()
    # `--` prefix marks a project row; exclude the `---` separators, which
    # would otherwise match and have no space to split on.
    project_lines = [
        line for line in lines if line.startswith("--") and not line.startswith("---")
    ]
    # ``--apple`` < ``--banana`` < ``--cherry`` on the project label means we
    # also need to pull out only the ``--name`` prefix and check its order.
    labels = [line[len("--"):line.index(" ")] for line in project_lines]
    assert labels == ["apple", "banana", "cherry"]


# ---------------------------------------------------------------------------
# 5. A clean repo (is_dirty=False) shows no ✎ marker.
# ---------------------------------------------------------------------------

def test_clean_repo_no_pencil_marker():
    project = Project(id="t-5", category="demo", 
        name="repo", path="/p/repo", status_bucket="active",
        git=GitState(is_repo=True, branch="main", is_dirty=False, uncommitted_files=0),
        agent=AgentState(state="idle"),
    )
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
        projects=(project,),
    )
    line = next(line for line in render_menubar(radar).splitlines() if line.startswith("--repo"))
    assert "✎" not in line
    assert line.endswith(" | href=file:///p/repo")


# ---------------------------------------------------------------------------
# 6. A dirty repo with zero uncommitted files shows no ✎ marker.
# ---------------------------------------------------------------------------

def test_dirty_with_no_uncommitted_files_no_pencil_marker():
    """The contract is "is_dirty AND uncommitted_files > 0" — both must hold."""
    project = Project(id="t-6", category="demo", 
        name="repo", path="/p/repo", status_bucket="active",
        git=GitState(is_repo=True, branch="main", is_dirty=True, uncommitted_files=0),
        agent=AgentState(state="idle"),
    )
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
        projects=(project,),
    )
    line = next(line for line in render_menubar(radar).splitlines() if line.startswith("--repo"))
    assert "✎" not in line


# ---------------------------------------------------------------------------
# 7. A non-repo project (is_repo=False) shows no branch segment.
# ---------------------------------------------------------------------------

def test_non_repo_no_branch_segment():
    project = Project(id="t-7", category="demo", 
        name="nogit", path="/p/nogit", status_bucket="active",
        git=GitState(is_repo=False, branch=None),
        agent=AgentState(state="idle"),
    )
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
        projects=(project,),
    )
    line = next(line for line in render_menubar(radar).splitlines() if line.startswith("--nogit"))
    # The body of the label is just "nogit" — no ` · ` separator anywhere in
    # the project line.
    assert " · " not in line
    assert line == "--nogit | href=file:///p/nogit"


# ---------------------------------------------------------------------------
# 8. An idle project shows no ● marker.
# ---------------------------------------------------------------------------

def test_idle_project_no_working_dot():
    project = Project(id="t-8", category="demo", 
        name="idle", path="/p/idle", status_bucket="active",
        git=GitState(is_repo=True, branch="main"),
        agent=AgentState(state="idle"),
    )
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
        projects=(project,),
    )
    line = next(line for line in render_menubar(radar).splitlines() if line.startswith("--idle"))
    assert "●" not in line


if __name__ == "__main__":  # pragma: no cover
    sys.exit(__import__("pytest").main([__file__, "-v"]))
