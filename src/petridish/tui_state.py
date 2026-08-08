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
    AGENT_RECENT_MAX_S,
    Project,
    Radar,
    STATUS_BUCKETS,
    agent_state_for_silence,
    to_utc,
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
# Column layout (extracted so petri's rows align like swab list's do)
# ---------------------------------------------------------------------------

#: Column headers for the four :func:`format_row` fields. Deliberately
#: excludes "bucket" — petri conveys that via bucket sections, not a column.
ROW_HEADERS = ["name", "agent", "branch", "dirty"]


def column_widths(rows: list[list[str]], headers: list[str] = ROW_HEADERS) -> list[int]:
    """Per-column max width across ``headers`` and every row.

    Compute this once over *all* rows to be displayed together (e.g. every
    bucket section) so columns line up across sections, not just within one.
    """
    widths = [len(h) for h in headers]
    for row in rows:
        for i, cell in enumerate(row):
            if i < len(widths):
                widths[i] = max(widths[i], len(cell))
    return widths


def pad_row(row: list[str], widths: list[int]) -> str:
    """Join ``row`` into one line, left-padding each cell to ``widths[i]``.

    Mirrors ``cli.py``'s ``_print_table`` join style (two-space gutter)."""
    return "  ".join(
        cell.ljust(widths[i]) if i < len(widths) else cell
        for i, cell in enumerate(row)
    )


# ---------------------------------------------------------------------------
# Agent-state indicator (glyph + named color; tui.py maps the name to an
# actual curses color pair — kept out of this file so it stays curses-free)
# ---------------------------------------------------------------------------

#: schema.py's AGENT_STATES is ("working", "recent", "idle") — that's the
#: full granularity the sensors currently produce (see IMPLEMENTATION_PLAN.md
#: F3: liveness is mtime-recency based, no finer state machine from
#: transcripts alone). A tool like pixtuoid showing reading/editing/thinking
#: is reading richer hook events this project doesn't sense yet.
AGENT_STATE_GLYPHS: dict[str, tuple[str, str]] = {
    "working": ("●", "green"),   # ●
    "recent": ("●", "yellow"),   # ●
    "idle": ("○", "dim"),        # ○
}


def agent_bulb(state: str) -> tuple[str, str]:
    """Return ``(glyph, color_name)`` for an agent state.

    Unknown states (schema drift, a bug upstream) fall back to the same
    hollow/dim glyph as ``idle`` rather than raising — this is cosmetic,
    not load-bearing, and should never crash the renderer.
    """
    return AGENT_STATE_GLYPHS.get(state, ("○", "dim"))


# ---------------------------------------------------------------------------
# Silence clock
#
# The load-bearing signal for watching unattended runs. ``agent.state`` cannot
# carry it: a delegate-to-local round where the local model thinks for four
# minutes reads "recent", which is indistinguishable from a run that wedged
# four minutes ago. The elapsed time since ``last_event_at``, recomputed at
# render time, is what actually separates them.
# ---------------------------------------------------------------------------

#: Silence beyond this is drawn as a warning rather than a state bulb. Same
#: boundary as ``idle`` — an agent that has a session but has not emitted an
#: event in half an hour is the thing you opened the dashboard to find.
STALL_AFTER_S = AGENT_RECENT_MAX_S


def format_silence(
    at: datetime | None,
    *,
    now: datetime,
    precise: bool = False,
) -> str:
    """Elapsed time since ``at`` as a compact human string.

    Two call sites want two densities, so this takes a flag rather than being
    two near-identical functions:

    * ``precise=False`` — one unit, for the narrow list column: ``12s``,
      ``47m``, ``3h``, ``41d``.
    * ``precise=True`` — two units, for the detail pane: ``4m 12s``,
      ``3h 04m``, ``41d 02h``. Sub-minute stays one unit; there is no
      meaningful second unit below it.

    ``at=None`` renders ``-`` (no agent has ever touched this project).
    Negative elapsed time — clock skew between the daemon that wrote the
    timestamp and the frontend reading it — clamps to zero rather than
    rendering a nonsensical negative age.
    """
    if at is None:
        return "-"
    elapsed = (to_utc(now) - to_utc(at)).total_seconds()
    if elapsed < 0:
        elapsed = 0.0

    secs = int(elapsed)
    if secs < 60:
        return f"{secs}s"

    minutes, s = divmod(secs, 60)
    if minutes < 60:
        return f"{minutes}m {s:02d}s" if precise else f"{minutes}m"

    hours, m = divmod(minutes, 60)
    if hours < 24:
        return f"{hours}h {m:02d}m" if precise else f"{hours}h"

    days, h = divmod(hours, 24)
    return f"{days}d {h:02d}h" if precise else f"{days}d"


def has_agent(project: Project) -> bool:
    """Whether an agent has ever been seen in this project.

    Keyed on ``last_event_at``, not ``session_id``: the silence clock and the
    stall glyph both need the timestamp, so its presence — not the id's — is
    what makes the richer rendering possible. ``scan.py`` deliberately keeps
    this metadata populated for idle projects.
    """
    return project.agent.last_event_at is not None


def silence_seconds(project: Project, *, now: datetime) -> float:
    """Seconds since this project's last agent event; ``inf`` if never.

    ``inf`` rather than ``None`` so it sorts as "quietest of all" without the
    caller needing a null branch in every comparison.
    """
    at = project.agent.last_event_at
    if at is None:
        return float("inf")
    return max(0.0, (to_utc(now) - to_utc(at)).total_seconds())


def glyph_for(project: Project, *, now: datetime) -> tuple[str, str]:
    """Return ``(glyph, color_name)`` for a project's live agent status.

    A three-way split, and deliberately **not** a function of
    ``project.agent.state``:

    * ``⚠ warn`` — an agent has been here but has been silent past
      :data:`STALL_AFTER_S`. Needs attention.
    * ``● green``/``● yellow`` — an agent is moving; the colour comes from
      re-deriving the state at *render* time, so it stays honest even when
      ``projects.json`` is a couple of minutes stale.
    * ``○ dim`` — no agent has ever been seen here.

    The stored ``state`` field is bypassed on purpose. It was stamped when the
    daemon last scanned, and this glyph sits next to a live silence counter;
    reading the stale field would let the two disagree on screen.
    """
    if not has_agent(project):
        return ("○", "dim")
    silence = silence_seconds(project, now=now)
    if silence >= STALL_AFTER_S:
        return ("⚠", "warn")
    return agent_bulb(agent_state_for_silence(silence))


# ---------------------------------------------------------------------------
# Dashboard ordering, density and pane layout
# ---------------------------------------------------------------------------

def sort_for_dashboard(projects: list[Project], *, now: datetime) -> list[Project]:
    """Order the dashboard's top section: the row that needs you comes first.

    Two groups, in this order:

    1. **Projects with an agent**, longest silence first. This is a triage
       order — sorting most-recent-first would put the healthiest run at the
       top and bury the wedged one, which defeats the whole point of the
       screen.
    2. **Projects without an agent**, most recent activity first. These are
       context, not triage, so the useful order flips: newest is most
       relevant. (They land in the top section at all because ``_bucket()``
       calls anything touched within 48h "active".)

    Ties break on ``name`` so the output is deterministic — otherwise two runs
    that last spoke in the same second could swap places between repaints and
    make the screen flicker.

    Implemented as **two partitions concatenated**, not one composite sort key.
    A single key wants a leading group rank plus a signed second element, and
    then the sign silently does the partitioning on its own — the rank becomes
    decorative and no test can tell whether it is there. Two explicit lists
    make the grouping load-bearing and readable.
    """
    def _age(p: Project) -> float:
        if p.last_activity_at is None:
            return float("inf")
        return max(0.0, (to_utc(now) - to_utc(p.last_activity_at)).total_seconds())

    with_agent = sorted(
        (p for p in projects if has_agent(p)),
        key=lambda p: (-silence_seconds(p, now=now), p.name),
    )
    without_agent = sorted(
        (p for p in projects if not has_agent(p)),
        key=lambda p: (_age(p), p.name),
    )
    return with_agent + without_agent


#: Card densities for the dashboard's top section.
DENSITIES = ("roomy", "compact")

#: Above this many rows in the top section, roomy cards stop fitting: 5 cards
#: x 4 lines would leave nothing for the in-flight and stale sections on a
#: 40-row terminal. Density collapses instead of capping the count — on an
#: overnight fan-out you want to see *every* run, not the top three.
ROOMY_MAX_ROWS = 4


def dashboard_density(n_rows: int, *, override: str | None = None) -> str:
    """Pick ``"roomy"`` or ``"compact"`` for ``n_rows`` projects.

    ``override`` is the user's ``z`` keypress and always wins — but an
    unrecognised value is ignored rather than raising, because this is a
    cosmetic choice reached from a key handler.
    """
    if override in DENSITIES:
        # Narrow to the literal type pyright infers from DENSITIES membership.
        return "compact" if override == "compact" else "roomy"
    return "roomy" if n_rows <= ROOMY_MAX_ROWS else "compact"


#: Terminal width at or above which the browser's detail pane sits on the
#: right. Below it, the pane goes back to the bottom. A side pane spends
#: columns you were not using; a bottom pane spends rows, which is the one
#: resource the project list needs.
DETAIL_RIGHT_MIN_COLS = 100


def detail_layout(width: int) -> str:
    """``"right"`` or ``"bottom"`` for the browser's detail pane.

    Derived from terminal width rather than bound to a key: that makes
    terminal-resize handling fall out for free, and leaves one fewer thing to
    remember.
    """
    return "right" if width >= DETAIL_RIGHT_MIN_COLS else "bottom"


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
    "ROW_HEADERS",
    "column_widths",
    "pad_row",
    "AGENT_STATE_GLYPHS",
    "agent_bulb",
    "STALL_AFTER_S",
    "format_silence",
    "has_agent",
    "silence_seconds",
    "glyph_for",
    "sort_for_dashboard",
    "DENSITIES",
    "ROOMY_MAX_ROWS",
    "dashboard_density",
    "DETAIL_RIGHT_MIN_COLS",
    "detail_layout",
]
