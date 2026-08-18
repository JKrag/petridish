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
    regression in header/footer/format immediately obvious. ``alpha`` has a
    live AI session, so it surfaces at the flat top level (not nested under
    "Active") and is excluded from the "Active" bucket entirely -- since
    alpha was the only active project, no "Active" header appears at all.
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
        'alpha · main ✎3 ● | href="file:///p/alpha"\n'
        "---\n"
        "Cold\n"
        '--beta | href="file:///p/beta"\n'
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
    assert line.endswith(' | href="file:///p/repo"')


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
    assert line == '--nogit | href="file:///p/nogit"'


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


# ---------------------------------------------------------------------------
# 9. A path containing a space is quoted in the href — xbar/SwiftBar split a
#    line's key=value parameters on whitespace, so an unquoted value with a
#    space (e.g. "~/Downloads/Kubernetes handin_639180485") breaks xbar's own
#    parser ("malformed parameters: missing equals") and disables the plugin.
#    Confirmed against a real xbar install, not just a spec reading.
# ---------------------------------------------------------------------------

def test_path_with_space_is_quoted_in_href():
    project = Project(id="t-9", category="demo",
        name="spacey", path="/p/has space/repo", status_bucket="active",
        git=GitState(is_repo=False),
        agent=AgentState(state="idle"),
    )
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
        projects=(project,),
    )
    line = next(line for line in render_menubar(radar).splitlines() if line.startswith("--spacey"))
    assert line == '--spacey | href="file:///p/has space/repo"'


# ---------------------------------------------------------------------------
# 10. Live-session restructure: any project with agent.state == "working"
#     surfaces flat at the top level, regardless of its status bucket, and
#     is excluded from that bucket's submenu so it never appears twice.
# ---------------------------------------------------------------------------

def test_working_project_surfaces_at_top_level_from_any_bucket():
    """A working project in the 'stale' bucket still surfaces at the flat
    top level, not nested under a 'Stale' header — and 'Stale' still shows
    its other (non-working) members normally."""
    working_but_stale = Project(id="t-10", category="demo",
        name="urgent", path="/p/urgent", status_bucket="stale",
        git=GitState(is_repo=True, branch="main"),
        agent=AgentState(state="working"),
    )
    idle_stale = Project(id="t-11", category="demo",
        name="quiet", path="/p/quiet", status_bucket="stale",
        agent=AgentState(state="idle"),
    )
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
        projects=(working_but_stale, idle_stale),
    )
    expected = (
        "🧫 1/2\n"
        "---\n"
        'urgent · main ● | href="file:///p/urgent"\n'
        "---\n"
        "Stale\n"
        '--quiet | href="file:///p/quiet"\n'
        "---\n"
        "Refresh | refresh=true"
    )
    assert render_menubar(radar) == expected


def test_no_live_projects_omits_top_level_section_and_divider():
    """With nobody working, the output is unchanged from the pre-restructure
    shape: no flat top-level entries, no extra divider before the first
    bucket header."""
    project = Project(id="t-12", category="demo",
        name="idle-one", path="/p/idle-one", status_bucket="active",
        agent=AgentState(state="idle"),
    )
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
        projects=(project,),
    )
    expected = (
        "🧫 0/1\n"
        "---\n"
        "Active\n"
        '--idle-one | href="file:///p/idle-one"\n'
        "---\n"
        "Refresh | refresh=true"
    )
    assert render_menubar(radar) == expected


def test_all_projects_working_omits_bucket_sections_and_mid_divider():
    """When every project is working, there are no bucket sections at all —
    just the flat top-level list, with no dangling mid-divider."""
    project = Project(id="t-13", category="demo",
        name="only-one", path="/p/only-one", status_bucket="active",
        agent=AgentState(state="working"),
    )
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
        projects=(project,),
    )
    expected = (
        "🧫 1/1\n"
        "---\n"
        'only-one ● | href="file:///p/only-one"\n'
        "---\n"
        "Refresh | refresh=true"
    )
    assert render_menubar(radar) == expected


# ---------------------------------------------------------------------------
# 11. parent_path is rendered as "{basename} / {project.name}" at the start
#     of every project line, regardless of section.
# ---------------------------------------------------------------------------

def test_parent_path_prefix_shown_in_live_section():
    """A project with a non-None parent_path shows the parent basename
    followed by ' / ' and its own name at the start of the project line."""
    project = Project(
        id="t-14", category="demo",
        name="child-repo", path="/p/child", status_bucket="active",
        parent_path="/Users/jan/repos/worktrees",
        git=GitState(is_repo=True, branch="feat-x"),
        agent=AgentState(state="working"),
    )
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
        projects=(project,),
    )
    # This project is working, so it surfaces at the flat top level, not in
    # the "Active" bucket.  The parent basename "worktrees" followed by
    # ' / ' and "child-repo" must appear at the start of the project line.
    expected = (
        "🧫 1/1\n"
        "---\n"
        'worktrees / child-repo · feat-x ● | href="file:///p/child"\n'
        "---\n"
        "Refresh | refresh=true"
    )
    assert render_menubar(radar) == expected


def test_parent_path_prefix_shown_in_bucket_section():
    """The same parent-prefix rule applies in a bucket (non-live) section —
    reuse the exact same layout a live-session test builds with."""
    parent_project = Project(
        id="t-15a", category="demo",
        name="child-repo", path="/p/child", status_bucket="cold",
        parent_path="/Users/jan/repos/worktrees",
        git=GitState(is_repo=True, branch="main"),
        agent=AgentState(state="idle"),
    )
    other = Project(
        id="t-15b", category="demo",
        name="standalone", path="/p/standalone", status_bucket="cold",
        agent=AgentState(state="idle"),
    )
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
        projects=(parent_project, other),
    )
    expected = (
        "🧫 0/2\n"
        "---\n"
        "Cold\n"
        '--worktrees / child-repo · main | href="file:///p/child"\n'
        '--standalone | href="file:///p/standalone"\n'
        "---\n"
        "Refresh | refresh=true"
    )
    assert render_menubar(radar) == expected


# ---------------------------------------------------------------------------
# 12. parent_path = None leaves the label as bare project.name — unchanged.
# ---------------------------------------------------------------------------

def test_no_parent_path_renders_bare_name():
    """When parent_path is None the function is identical to the pre-change
    behaviour: bare project.name, with the usual suffixes, no prefix."""
    project = Project(
        id="t-16", category="demo",
        name="standalone", path="/p/standalone", status_bucket="active",
        git=GitState(is_repo=True, branch="main", is_dirty=True, uncommitted_files=2),
        agent=AgentState(state="idle"),
    )
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
        projects=(project,),
    )
    # Verify the project line exists and contains no " / " inside the label
    # (only in the bucket header/footer context, never in a name).
    line = next(line for line in render_menubar(radar).splitlines()
                if line.startswith("--standalone"))
    assert line == '--standalone · main ✎2 | href="file:///p/standalone"'
    # Also confirm via the bucket-not-present check: there's no live
    # section header since it's not working, but "Active" must appear
    # before the project line.
    assert "Active" in render_menubar(radar)


# ---------------------------------------------------------------------------
# 13. parent-prefix combines with existing suffixes (branch, dirty, ●).
# ---------------------------------------------------------------------------

def test_parent_prefix_with_all_suffixes():
    """A worktree project that is dirty and working must show both the
    branch marker and the pencil marker alongside the parent-prefix."""
    project = Project(
        id="t-17", category="demo",
        name="worktree-repo", path="/p/wr", status_bucket="active",
        parent_path="/Users/jan/repos/catshow-searcher",
        git=GitState(is_repo=True, branch="feat-x", is_dirty=True, uncommitted_files=3),
        agent=AgentState(state="working"),
    )
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
        projects=(project,),
    )
    # Both the "working" ● and the dirty ✎3 and branch markers should
    # appear after "catshow-searcher / worktree-repo", and the project
    # must surface at the flat top level.
    expected = (
        "🧫 1/1\n"
        "---\n"
        'catshow-searcher / worktree-repo · feat-x ✎3 ● | href="file:///p/wr"\n'
        "---\n"
        "Refresh | refresh=true"
    )
    assert render_menubar(radar) == expected


def test_parent_prefix_combines_with_non_working_project():
    """When the agent is not working, only branch + dirty suffixes apply;
    the ● marker must not appear."""
    project = Project(
        id="t-18", category="demo",
        name="cold-worktree", path="/p/cw", status_bucket="cold",
        parent_path="/Users/jan/repos/monorepo",
        git=GitState(is_repo=True, branch="develop", is_dirty=True, uncommitted_files=1),
        agent=AgentState(state="idle"),
    )
    other = Project(
        id="t-18b", category="demo",
        name="other", path="/p/other", status_bucket="cold",
        agent=AgentState(state="idle"),
    )
    radar = Radar(
        updated_at=datetime(2026, 1, 1, 0, 0, 0, tzinfo=timezone.utc),
        projects=(project, other),
    )
    # Parent-prefix + branch + dirty, no ●.  The ``--`` bucket prefix is
    # added by the caller, then the label itself begins with the parent
    # basename.  We assert the full bucket-row content.
    line = next(line for line in render_menubar(radar).splitlines()
                if "monorepo / cold-worktree" in line)
    assert "monorepo / cold-worktree · develop ✎1" in line
    assert "●" not in line


if __name__ == "__main__":  # pragma: no cover
    sys.exit(__import__("pytest").main([__file__, "-v"]))
