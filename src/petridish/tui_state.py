"""TUI-facing state helpers for the petri dish project-crawler.

This module is intentionally ``curses``-free: it knows nothing about terminal
dimensions, colours, or key events. It turns an in-memory :class:`Radar`
into bucketed, filtered, formatted rows and tracks an interaction cursor —
the TUI layer (M11 / M12) is responsible for rendering them.

Everything here runs on stdlib-only and can be exercised by ``pytest`` with
the terminal fully closed.  That is the contract tests rely on.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime

from petridish.schema import (
    Project,
    Radar,
    STATUS_BUCKETS,
)


# ---------------------------------------------------------------------------
# Grouping & filtering
# ---------------------------------------------------------------------------

def group_by_bucket(
    projects: list[Project],
) -> dict[str, list[Project]]:
    """Bucket ``projects`` by ``status_bucket``, preserving ``STATUS_BUCKETS``
    order and omitting foreign projects.

    Every bucket name from :data:`STATUS_BUCKETS` appears as a key (mapped to
    an empty list when absent) so callers can iterate without testing for
    ``KeyError`` — the TUI renders all four rows regardless.
    """
    buckets: dict[str, list[Project]] = {b: [] for b in STATUS_BUCKETS}
    for p in projects:
        if p.is_foreign:
            continue
        buckets[p.status_bucket].append(p)
    return buckets


def filter_projects(
    projects: list[Project],
    query: str,
) -> list[Project]:
    """Case-insensitive substring filter on :attr:`Project.name`.

    An empty query returns every project verbatim (rather than the whole
    set) — matching :meth:`filter_projects`'s contract with ``swab path``.
    """
    if not query:
        return list(projects)
    needle = query.lower()
    return [p for p in projects if needle in p.name.lower()]


# ---------------------------------------------------------------------------
# Row / detail rendering
# ---------------------------------------------------------------------------

def format_row(project: Project) -> list[str]:
    """Produce the four TUI columns for one project.

    Mirrors :func:`petridish.cli._print_table` row-building logic — the agent
    label and the dirty marker come from exactly the same expressions:

    * ``[name, agent_label, branch_or_dash, dirty_marker]``
    """
    agent_label = (
        f"{project.agent.active_agent} ({project.agent.state})"
        if project.agent.active_agent
        else project.agent.state
    )
    branch = project.git.branch or "-"
    dirty = "*" if (project.git.is_repo and project.git.is_dirty) else " "
    return [
        project.name,
        agent_label,
        branch,
        dirty,
    ]


def format_detail(project: Project) -> list[str]:
    """Return a ``label: value`` line for every interesting field of
    ``project``.  ``None`` values render as the literal string ``"none"`` so
    the detail panel never crashes and stays stable as fields evolve.

    ``mine_last_commit_at`` is omitted when identical to :attr:`last_commit_at`
    so the line doesn't just repeat itself.  Order matters to TUI callers
    (path is the headline, then git, then agent).
    """

    def _v(value: object) -> str:
        return "none" if value is None else str(value)

    lines: list[str] = []
    lines.append(f"path: {_v(project.path)}")
    lines.append(f"branch: {_v(project.git.branch)}")
    uncommitted = project.git.uncommitted_files or 0
    lines.append(f"dirty files: {uncommitted}")

    last_commit = _v(project.git.last_commit_at)
    lines.append(f"last commit at: {last_commit}")

    mine = _v(project.git.mine_last_commit_at)
    if project.git.mine_last_commit_at != project.git.last_commit_at:
        lines.append(f"mine last commit at: {mine}")

    lines.append(f"github url: {_v(project.git.github_url)}")
    lines.append(f"agent state: {_v(project.agent.state)}")
    lines.append(f"active agent: {_v(project.agent.active_agent)}")
    lines.append(f"session id: {_v(project.agent.session_id)}")
    lines.append(f"last activity at: {_v(project.last_activity_at)}")
    return lines


# ---------------------------------------------------------------------------
# Interaction cursor
# ---------------------------------------------------------------------------

@dataclass
class SelectionState:
    """Which bucket + row the TUI cursor is on.

    Both fields are plain strings / ints on purpose — nothing here talks to
    the terminal, so the cursor can be serialised, reset to defaults, and
    inspected in tests without importing anything terminal-related.
    """

    bucket: str = ""
    index: int = 0


def move(state: SelectionState, delta: int, grouped: dict[str, list[Project]]) -> SelectionState:
    """Shift the cursor by ``delta`` rows across all buckets in
    :data:`STATUS_BUCKETS` order.

    Buckets are flattened in order — moving off the last row of ``active``
    lands on the first row of the next non-empty bucket below it, and empty
    buckets contribute zero positions.  The cursor is clamped to the first
    bucket's first row at the top and the last bucket's last row at the
    bottom: it never wraps around.

    Returns a new :class:`SelectionState`; the input is left untouched.
    When ``grouped`` contains no projects at all (empty dict or every bucket
    empty), the returned cursor points at nothing meaningful and does not
    raise.
    """
    buckets_in_order = [b for b in STATUS_BUCKETS if b in grouped]

    # Flatten rows: each entry is ``(bucket, index_within_bucket)``.
    rows: list[tuple[str, int]] = []
    for bucket in buckets_in_order:
        for i, _project in enumerate(grouped[bucket]):
            rows.append((bucket, i))

    if not rows:
        return SelectionState(bucket="", index=0)

    # Locate current position in the flattened sequence.
    try:
        current_index = rows.index((state.bucket, state.index))
    except ValueError:
        current_index = 0

    target = current_index + delta
    target = max(0, min(target, len(rows) - 1))

    chosen_bucket, chosen_index = rows[target]
    return SelectionState(bucket=chosen_bucket, index=chosen_index)


def selected_project(
    state: SelectionState,
    grouped: dict[str, list[Project]],
) -> Project | None:
    """Return the project under the cursor, or ``None`` if no project
    resolves to that selection (e.g. when every bucket is empty)."""
    bucket_rows = grouped.get(state.bucket)
    if not bucket_rows:
        return None
    if state.index < 0 or state.index >= len(bucket_rows):
        return None
    return bucket_rows[state.index]


# ---------------------------------------------------------------------------
# Staleness check (pure: no ``datetime.now`` inside)
# ---------------------------------------------------------------------------

def is_stale(
    radar: Radar,
    *,
    now: datetime,
    threshold_hours: float = 24.0,
) -> bool:
    """Return ``True`` if the radar is at least ``threshold_hours`` old.

    ``now`` is supplied by the caller so the check stays pure and easy to
    exercise — tests pin it, the TUI pins it to its own clock.  A no-op on
    an empty radar (``updated_at`` is required and always present) but the
    function never raises for caller inputs.
    """
    elapsed_seconds = (now - radar.updated_at).total_seconds()
    elapsed_hours = elapsed_seconds / 3600.0
    return elapsed_hours >= threshold_hours


__all__ = [
    "group_by_bucket",
    "filter_projects",
    "format_row",
    "format_detail",
    "SelectionState",
    "move",
    "selected_project",
    "is_stale",
]
