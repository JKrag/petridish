"""Render a menubar text string for xbar/SwiftBar from an :class:`Radar`.

Pure function: one input, one output, no side effects. Every consumer
(``swab``, Raycast previews, any future hook) goes through this same format,
so keeping it in one place makes the contract easy to test and extend.

Output format — four sections joined with ``\\n``:

1. Menu bar title line: ``🧫 {working}/{total}``.
2. Project sections, ordered by bucket (active → in_flight → stale → cold);
   each bucket emits a header followed by one line per project, sorted by
   ``project.name``.  Projects that are not in any of the four buckets are
   dropped — ``STATUS_BUCKETS`` is the universe of emitable labels.
3. Footer divider and a manual-refresh link.

An entirely empty radar produces the same four-section shape with a single
``No projects`` placeholder in place of the project sections.

No file I/O, no clock reads, no environment access.  ``render_menubar``
operates on the frozen :class:`Radar` dataclasses only.
"""

from __future__ import annotations

from petridish.schema import Project, Radar, STATUS_BUCKETS

_LABELS: tuple[str, ...] = (
    "Active",
    "In flight",
    "Stale",
    "Cold",
)

def _project_label(project: Project) -> str:
    """Format the ``--{label}`` prefix for one project line.

    Fields are concatenated in this exact order: ``name``, optional `` · branch``
    (only when the project is a git repo *and* has a branch), optional `` ✎N``
    (only when dirty *and* there are uncommitted files), optional `` ●`` (only
    when the agent reports ``working``).  The four components are joined with no
    separator — each component carries its own leading space when present, and
    the trailing `` | href=...`` is appended by the caller.
    """
    git = project.git
    agent = project.agent

    parts: list[str] = [project.name]

    if git.is_repo and git.branch:
        parts.append(" · " + git.branch)

    if git.is_dirty and git.uncommitted_files > 0:
        parts.append(" ✎" + str(git.uncommitted_files))

    if agent.state == "working":
        parts.append(" ●")

    return "".join(parts)


def _render_project_section() -> str:
    """The section between the two dividers.  ``"No projects"`` when empty."""
    return "No projects | color=#888888"


def render_menubar(radar: Radar) -> str:
    """Build the xbar/SwiftBar menubar text for *radar*.

    See module docstring for the exact shape of the returned string. The
    contract is **the function's public surface** — callers do not reach past
    this function into the bucket layout or project formatting.
    """
    total = len(radar.projects)
    working = sum(1 for p in radar.projects if p.agent.state == "working")
    lines: list[str] = [f"🧫 {working}/{total}", "---"]

    if total == 0:
        lines.append(_render_project_section())
    else:
        for status_bucket, label in zip(STATUS_BUCKETS, _LABELS):
            bucket_projects = tuple(
                p for p in radar.projects if p.status_bucket == status_bucket
            )
            if not bucket_projects:
                continue
            lines.append(label)
            for project in sorted(bucket_projects, key=lambda p: p.name):
                lines.append(f"--{_project_label(project)} | href=file://{project.path}")

    lines.append("---")
    lines.append("Refresh | refresh=true")

    return "\n".join(lines)


__all__ = ["render_menubar"]
