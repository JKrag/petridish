"""Render a menubar text string for xbar/SwiftBar from an :class:`Radar`.

Pure function: one input, one output, no side effects. Every consumer
(``swab``, Raycast previews, any future hook) goes through this same format,
so keeping it in one place makes the contract easy to test and extend.

Output format — up to four sections joined with ``\\n``:

1. Menu bar title line: ``🧫 {working}/{total}``.
2. **Live-session section** (flat, top-level — no bucket header, no ``--``
   indent): every project with ``agent.state == "working"``, sorted by
   ``project.name``, regardless of which status bucket it's in. These are
   the projects with an active AI session running right now — surfaced
   directly at the top level of the dropdown for instant access, not buried
   in a submenu. Present only when at least one project is working.
3. Bucket sections, ordered active → in_flight → stale → cold; each bucket
   emits a header followed by one indented (``--``) line per project, sorted
   by ``project.name`` — **excluding** any project already shown in the
   live-session section above, so a working project never appears twice.
   Projects not in any of the four buckets are dropped — ``STATUS_BUCKETS``
   is the universe of emitable labels. A ``---`` divider separates the
   live-session section from the bucket sections when both are non-empty.
   Each project line's ``href`` value is double-quoted (``href="file://..."``)
   — xbar/SwiftBar split a line's ``key=value`` parameters on whitespace, so
   an unquoted value containing a space (a project path like
   ``~/Downloads/Kubernetes handin_639180485``) breaks xbar's own parser with
   a "malformed parameters: missing equals" error and disables the plugin.
4. Footer divider and a manual-refresh link.

An entirely empty radar produces the same shape with a single ``No
projects`` placeholder in place of the live-session/bucket sections.

No file I/O, no clock reads, no environment access.  ``render_menubar``
operates on the frozen :class:`Radar` dataclasses only.
"""

from __future__ import annotations

import os

from petridish.schema import Project, Radar, STATUS_BUCKETS

_LABELS: tuple[str, ...] = (
    "Active",
    "In flight",
    "Stale",
    "Cold",
)

def _project_label(project: Project) -> str:
    """Format the ``--{label}`` prefix for one project line.

    Fields are concatenated in this exact order: if ``parent_path`` is set the
    leading text is ``{parent_basename} / {project.name}``; otherwise it is
    bare ``project.name``.  Then optional `` · branch``, optional `` ✎N`` (only
    when dirty *and* there are uncommitted files), and optional `` ●`` (only
    when the agent reports ``working``) are appended.  The components are
    joined with no separator — each component carries its own leading space
    when present, and the trailing `` | href=...`` is appended by the caller.
    """
    git = project.git
    agent = project.agent

    parts: list[str] = []
    if project.parent_path is not None:
        parts.append(os.path.basename(project.parent_path))
        parts.append(" / ")
    parts.append(project.name)

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
        live_projects = tuple(p for p in radar.projects if p.agent.state == "working")
        for project in sorted(live_projects, key=lambda p: p.name):
            lines.append(f'{_project_label(project)} | href="file://{project.path}"')

        live_ids = {p.id for p in live_projects}
        bucket_lines: list[str] = []
        for status_bucket, label in zip(STATUS_BUCKETS, _LABELS):
            bucket_projects = tuple(
                p for p in radar.projects
                if p.status_bucket == status_bucket and p.id not in live_ids
            )
            if not bucket_projects:
                continue
            bucket_lines.append(label)
            for project in sorted(bucket_projects, key=lambda p: p.name):
                bucket_lines.append(f'--{_project_label(project)} | href="file://{project.path}"')

        if live_projects and bucket_lines:
            lines.append("---")
        lines.extend(bucket_lines)

    lines.append("---")
    lines.append("Refresh | refresh=true")

    return "\n".join(lines)


__all__ = ["render_menubar"]
