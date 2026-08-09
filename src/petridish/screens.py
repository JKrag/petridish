"""The two petri screens, as pure functions.

``petri`` is two screens with different jobs, and the split is why neither has
to compromise:

* **Dashboard** (:func:`render_dashboard`) is an *ambient monitor*. You leave it
  open in a pane and look at it, not with it. Its job is answering "does
  anything need me?" across a fleet of unattended agent runs. It has no cursor
  and takes no input.
* **Browser** (:func:`render_browser`) is a *tool you drive*. Hands on keys,
  searching, selecting, opening, resuming. Density beats prominence and every
  project must be reachable.

Both return ``list[str]`` — at most ``height`` lines, each at most ``width``
columns — so ``tui.py`` is a blitter with no layout logic of its own. That is
what keeps the "reserved rows" arithmetic that used to live in the render loop
out of the terminal layer, and it is what makes both screens snapshot-testable
with the display closed.

**No I/O, no clock, no environment.** ``now`` and ``home`` are injected. The
dashboard's ambient nature is exactly why: with no cursor and no input it is a
pure function, which buys a third consumer for free — a non-interactive
``swab dash`` that prints the same block once and exits, pipeable into a tmux
status pane.
"""

from __future__ import annotations

from datetime import datetime

from petridish.schema import Project, QuotaState, Radar, to_utc
from petridish.tui_state import (
    ROW_HEADERS,
    column_widths,
    filter_projects,
    format_countdown,
    format_detail,
    format_silence,
    glyph_for,
    group_by_bucket,
    has_agent,
    pad_row,
    sort_for_dashboard,
)

#: Bucket labels as they appear on screen.
_BUCKET_LABELS = {
    "active": "RUNNING",
    "in_flight": "IN FLIGHT",
    "stale": "STALE",
    "cold": "COLD",
}

#: Lines a roomy card occupies, including its trailing blank separator.
ROOMY_CARD_HEIGHT = 4

#: Keymap footers. These advertise **only keys tui.py actually binds** — a
#: footer promising "⏎ open" or "r resume" when nothing is wired to them is
#: worse than a shorter footer, because the user learns to distrust the whole
#: line. Extend these in the same commit that binds the key.
DASHBOARD_KEYS = " tab browser   z density   q quit"
BROWSER_KEYS = " tab dashboard   j/k move   / search   esc clear   q quit"

#: Column headers for the browser's project table. Extends the shared
#: ``ROW_HEADERS`` with the silence column, which only the new screens show.
BROWSER_HEADERS = [*ROW_HEADERS[:-1], "✎", "silent"]


# ---------------------------------------------------------------------------
# Text fitting
# ---------------------------------------------------------------------------

def _fit(text: str, n: int) -> str:
    """Truncate ``text`` to ``n`` columns, marking loss with an ellipsis.

    Truncating is always preferable to letting a long branch name or path wrap:
    curses ``addnstr`` would clip it anyway, but silently, and a wrapped line
    would shift every row below it out of alignment.
    """
    if n <= 0:
        return ""
    if len(text) <= n:
        return text
    if n == 1:
        return "…"
    return text[: n - 1] + "…"


def _split(left: str, right: str, width: int) -> str:
    """One line with ``left`` flush left and ``right`` flush right.

    Returns exactly ``width`` columns when ``right`` is non-empty, so a caller
    can append a vertical rule at a fixed column. ``left`` yields ground when
    the two would collide — the right-hand side carries the volatile values
    (silence, counts) that the eye is actually tracking.
    """
    if not right:
        return _fit(left, width)
    left = _fit(left, max(0, width - len(right) - 1))
    return left.ljust(max(0, width - len(right))) + right


def _clip(lines: list[str], *, width: int, height: int) -> list[str]:
    """The budget guard, applied once at each renderer's exit.

    Every caller could ``_fit`` its own line, and the first version did — which
    meant one missed call site (the compact row, at width 40) silently broke the
    invariant ``tui.py`` depends on. Enforcing it structurally at the boundary
    is the difference between a property that holds and one that mostly holds.
    """
    return [_fit(line, width) for line in lines[:height]]


def _tilde(path: str, home: str | None) -> str:
    """Collapse ``home`` to ``~``. ``home=None`` leaves the path untouched.

    Injected rather than read from the environment so the renderers stay pure;
    ``tui.py`` passes ``os.path.expanduser("~")``.
    """
    if not home:
        return path
    if path == home:
        return "~"
    if path.startswith(home.rstrip("/") + "/"):
        return "~" + path[len(home.rstrip("/")) :]
    return path


def _dirty(project: Project) -> str:
    """``✎N`` for a dirty repo with uncommitted files, else empty.

    Both conditions must hold — the same contract ``menubar.py`` uses, so the
    two frontends can't disagree about what "dirty" looks like.
    """
    git = project.git
    if git.is_repo and git.is_dirty and git.uncommitted_files > 0:
        return f"✎{git.uncommitted_files}"
    return ""


def _agent_label(project: Project) -> str:
    return project.agent.active_agent or ""


def _event_label(project: Project) -> str:
    """The last tool call, falling back to commit age when unavailable.

    ``last_event`` is only ever set by the hook path (``events.py``); both
    filesystem sensors construct their signal with ``event=None``. On a real
    ``projects.json`` it was populated **zero** times out of 76 projects, which
    made this column permanently blank — dead space in the compact row and an
    empty right-hand side on every roomy card.

    So it degrades to something the scanner does produce. Blank is only
    correct when there is genuinely nothing to say.
    """
    if project.agent.last_event:
        return project.agent.last_event
    if project.git.last_commit_at is not None:
        return ""  # caller supplies commit age; see _commit_label
    return ""


def _commit_label(project: Project, *, now: datetime) -> str:
    """``commit 3h ago`` (plus ``(you)`` when it is yours), or empty."""
    if project.git.last_commit_at is None:
        return ""
    age = format_silence(project.git.last_commit_at, now=now)
    mine = project.git.mine_last_commit_at == project.git.last_commit_at
    return f"commit {age} ago" + (" (you)" if mine else "")


# ---------------------------------------------------------------------------
# Dashboard cards
# ---------------------------------------------------------------------------

def format_card(
    project: Project,
    *,
    now: datetime,
    width: int,
    home: str | None = None,
) -> list[str]:
    """Three lines describing one project mid-run, plus a blank separator.

    The roomy density. Each line pairs a stable left-hand value with the
    volatile right-hand one you are actually scanning:

    1. glyph + name  ·  silence + which agent
    2. branch + dirty count  ·  last tool call
    3. path  ·  session id (what ``claude --resume`` needs)

    A project with no agent spends the same three lines on git facts instead,
    so the card layout survives the idle-but-recent case that ``_bucket()``
    puts in this section.
    """
    glyph, _color = glyph_for(project, now=now)
    lines: list[str] = []

    if has_agent(project):
        silence = format_silence(project.agent.last_event_at, now=now)
        agent = _agent_label(project)
        right1 = f"silent {silence}" + (f" · {agent}" if agent else "")
        session = project.agent.session_id
        right3 = f"sess {session[:18]}" if session else ""
        # No event to show is the common case, not the exception — fall back to
        # git rather than leaving half the card blank.
        right2 = _event_label(project) or _commit_label(project, now=now)
    else:
        right1 = "no agent"
        right2 = _commit_label(project, now=now)
        right3 = ""

    branch = project.git.branch or "-"
    dirty = _dirty(project)
    left2 = f"     {branch}" + (f"  {dirty}" if dirty else "")

    lines.append(_split(f" {glyph} {project.name}", right1, width))
    lines.append(_split(left2, right2, width))
    lines.append(_split(f"     {_tilde(project.path, home)}", right3, width))
    lines.append("")
    return lines


def format_compact_row(
    project: Project,
    *,
    now: datetime,
    width: int,
    name_w: int,
    branch_w: int,
) -> str:
    """One line describing a project mid-run — the compact density.

    Eight roomy cards would be 32 lines and evict every other section from a
    40-row terminal, so above the roomy ceiling the density collapses rather
    than the count being capped: on an overnight fan-out you want to see every
    run, not the top few. Column widths are supplied by the caller so rows line
    up across the whole section.
    """
    glyph, _color = glyph_for(project, now=now)
    silence = (
        format_silence(project.agent.last_event_at, now=now)
        if has_agent(project)
        else "-"
    )
    prefix = " ".join(
        [
            f" {glyph}",
            _fit(project.name, name_w).ljust(name_w),
            _fit(project.git.branch or "-", branch_w).ljust(branch_w),
            _dirty(project).rjust(4),
            silence.rjust(6),
        ]
    )
    # The event is left-aligned in whatever is left rather than flushed right:
    # a ragged right edge on the one free-text column reads as noise next to
    # five aligned ones.
    detail = _event_label(project) or _commit_label(project, now=now)
    event = _fit(detail, max(0, width - len(prefix) - 3))
    return f"{prefix}  {event}".rstrip()


# ---------------------------------------------------------------------------
# Shared section chrome
# ---------------------------------------------------------------------------

#: Cells in a quota bar. Ten keeps each cell worth exactly 10%, so the bar and
#: the percentage beside it can never appear to disagree.
QUOTA_BAR_CELLS = 10

#: Past this, the quota figures are labelled with their age. Claude Code only
#: rewrites its status file while a session is running, so overnight these
#: numbers go stale while the rest of projects.json stays fresh — a percentage
#: with no age next to it would quietly lie.
QUOTA_STALE_AFTER_S = 15 * 60


def _bar(pct: int | None, cells: int = QUOTA_BAR_CELLS) -> str:
    """A ``████░░░░░░`` meter. ``None`` yields all-empty."""
    if pct is None:
        return "░" * cells
    filled = max(0, min(cells, round(pct / 100 * cells)))
    return "█" * filled + "░" * (cells - filled)


def format_quota_line(
    quota: QuotaState | None, *, now: datetime, width: int
) -> list[str]:
    """The header's quota line, or ``[]`` when there is nothing to say.

    Returns a list so the caller can splice it in without a null check — an
    absent sensor simply contributes no rows.

    Degrades by width rather than truncating: the bars are the first thing
    dropped, because the percentage carries the same information in fewer
    columns. Below that, only the seven-day window survives — it is the one
    that bites, since the five-hour window refills on its own.
    """
    # These two guards overlap: a None quota would fall through to the second
    # one anyway (an empty QuotaState has no windows), so mutation testing
    # reports removing the first as an equivalent mutant. Kept for readability —
    # "no sensor reading" and "a reading with nothing in it" are different
    # facts, even when they render the same.
    if quota is None:
        return []

    five = quota.five_hour_used_pct
    seven = quota.seven_day_used_pct
    if five is None and seven is None:
        return []

    age = ""
    if quota.measured_at is not None:
        elapsed = (to_utc(now) - to_utc(quota.measured_at)).total_seconds()
        if elapsed >= QUOTA_STALE_AFTER_S:
            age = f"  · {format_silence(quota.measured_at, now=now)} old"

    def part(label: str, pct: int | None, resets: datetime | None, bars: bool) -> str:
        shown = "  -%" if pct is None else f"{pct:3d}%"
        meter = f"{_bar(pct)} " if bars else ""
        return f"{label} {meter}{shown}  resets {format_countdown(resets, now=now)}"

    for bars, both in ((True, True), (False, True), (False, False)):
        chunks = [part("7d", seven, quota.seven_day_resets_at, bars)]
        if both:
            chunks.insert(
                0, part("5h", five, quota.five_hour_resets_at, bars)
            )
        line = " " + "   ·   ".join(chunks) + age
        if len(line) <= width:
            return [line]

    # Even the bar-less single window does not fit; let the caller's clip
    # handle it rather than returning nothing at all.
    return [" " + part("7d", seven, quota.seven_day_resets_at, False)]


def _header(
    radar: Radar, *, now: datetime, width: int, title: str, n_projects: int
) -> list[str]:
    """Title line plus its heavy rule.

    ``n_projects`` is passed in rather than read off ``radar`` because it must
    be the *visible* count. Using ``len(radar.projects)`` made the header claim
    16 projects over a list of 15 whenever a foreign project was present —
    ``group_by_bucket`` drops those from the rows below.
    """
    right = (
        f"{n_projects} projects · "
        f"{now.strftime('%H:%M')} · "
        f"{radar.scan_duration_ms / 1000:.2f}s"
    )
    return [
        _split(f"petri · {title}", right, width).rstrip(),
        *format_quota_line(radar.quota, now=now, width=width),
        "═" * width,
    ]


def _section(
    label: str, count: int, width: int, *, note: str = "", rule_above: bool = False
) -> list[str]:
    """A section label with its count, between light rules.

    ``rule_above`` is off for the first section, which already sits under the
    header's heavy rule, and on for every subsequent one.
    """
    right = f"{count}" + (f" · {note}" if note else "")
    lines = ["─" * width] if rule_above else []
    lines.append(_split(f" {label}", right, width).rstrip())
    lines.append("─" * width)
    return lines


def browser_groups(radar: Radar, query: str = "") -> dict[str, list[Project]]:
    """The browser's projects, filtered and bucketed.

    Public because ``tui.py`` needs the *same* grouping to resolve its cursor
    into a ``Project``. If it rebuilt the grouping itself, a change to the
    filter here would silently desync the highlighted row from the selected
    project.
    """
    return group_by_bucket(filter_projects(_visible(radar), query))


def _visible(radar: Radar) -> list[Project]:
    """Non-foreign projects.

    ``group_by_bucket`` drops ``is_foreign`` from the rows on its own, so this
    exists for the *header count*: without it the "N projects" figure counted
    projects that were never listed below it.
    """
    return [p for p in radar.projects if not p.is_foreign]


# ---------------------------------------------------------------------------
# Screen 1: the dashboard
# ---------------------------------------------------------------------------

def render_dashboard(
    radar: Radar,
    *,
    now: datetime,
    width: int = 78,
    height: int = 40,
    density: str = "roomy",
    home: str | None = None,
) -> list[str]:
    """Render the ambient monitor: what is running, what is in flight, the rest.

    ``density`` is ``"roomy"`` or ``"compact"`` — callers get it from
    :func:`~petridish.tui_state.dashboard_density`, which is where the
    count-based default and the user's ``z`` override are reconciled.

    Returns at most ``height`` lines. Sections are emitted in priority order
    and simply stop when the budget runs out, so a short terminal loses the
    cold projects rather than the stalled run at the top.
    """
    visible = _visible(radar)
    buckets = group_by_bucket(visible)
    running = sort_for_dashboard(buckets["active"], now=now)

    lines = _header(
        radar, now=now, width=width, title="dashboard", n_projects=len(visible)
    )

    # "RUNNING" overstates it when nothing has an agent — the section is then
    # just the most recently touched projects.
    label = "RUNNING" if any(has_agent(p) for p in running) else "RECENT"
    note = "quietest first" + (" · compact" if density == "compact" else "")
    lines += _section(label, len(running), width, note=note)

    if not running:
        lines.append(" nothing active")
    elif density == "compact":
        name_w = min(18, max((len(p.name) for p in running), default=4))
        branch_w = min(15, max((len(p.git.branch or "-") for p in running), default=6))
        for project in running:
            lines.append(
                format_compact_row(
                    project, now=now, width=width, name_w=name_w, branch_w=branch_w
                )
            )
        lines.append("")
    else:
        for project in running:
            lines += format_card(project, now=now, width=width, home=home)

    in_flight = buckets["in_flight"]
    if in_flight:
        lines += _section("IN FLIGHT", len(in_flight), width, rule_above=True)
        lines += _git_rows(in_flight, now=now, width=width)
        lines.append("")

    rest = buckets["stale"] + buckets["cold"]
    if rest:
        n_stale, n_cold = len(buckets["stale"]), len(buckets["cold"])
        lines += _section(
            "STALE",
            n_stale,
            width,
            note=f"COLD {n_cold}" if n_cold else "",
            rule_above=True,
        )
        lines += _git_rows(buckets["stale"], now=now, width=width)
        if n_cold:
            names = " · ".join(p.name for p in buckets["cold"])
            lines.append(_fit(f"   {names}", width))

    lines.append("─" * width)
    lines.append(DASHBOARD_KEYS)
    return _clip(lines, width=width, height=height)


def _git_rows(projects: list[Project], *, now: datetime, width: int) -> list[str]:
    """One line per project, git-centric — no agent column.

    Safe by construction for ``in_flight``: ``_bucket()`` returns ``"active"``
    whenever the agent state is working or recent, *before* it looks at age, so
    an in-flight project can never have a running agent. The column width that
    would carry agent state is spent on commit age instead.
    """
    rows: list[str] = []
    for project in sorted(projects, key=lambda p: p.name):
        branch = _fit(project.git.branch or "-", 15).ljust(15)
        commit = (
            format_silence(project.git.last_commit_at, now=now)
            if project.git.last_commit_at
            else "-"
        )
        gh = "gh" if project.git.github_url else "—"
        left = f"   {_fit(project.name, 20).ljust(20)} {branch} {_dirty(project).rjust(4)}"
        rows.append(_split(left, f"commit {commit.rjust(4)}   {gh}", width).rstrip())
    return rows


# ---------------------------------------------------------------------------
# Screen 2: the browser
# ---------------------------------------------------------------------------

def format_browser_row(project: Project, *, now: datetime) -> list[str]:
    """The browser's table cells for one project.

    Deliberately built from the same helpers the dashboard uses — ``glyph_for``
    for the bulb, ``_dirty`` for the marker, ``format_silence`` for the clock —
    so the two screens cannot drift into two idioms for the same fact.
    """
    glyph, _color = glyph_for(project, now=now)
    silence = (
        format_silence(project.agent.last_event_at, now=now)
        if has_agent(project)
        else "-"
    )
    return [
        f"{glyph} {project.name}",
        _agent_label(project) or "-",
        project.git.branch or "-",
        _dirty(project).lstrip("✎") or "0",
        silence,
    ]


def format_detail_compact(
    project: Project, *, now: datetime, width: int, home: str | None = None
) -> list[str]:
    """Three dense lines for the bottom pane on a narrow terminal.

    ``format_detail`` emits about ten lines, which at this width competes with
    the project list for the one resource it needs — rows. So this carries only
    what a table row cannot: the full path, the session id, the last event.
    """
    branch = project.git.branch or "-"
    dirty = _dirty(project)
    silence = (
        f"silent {format_silence(project.agent.last_event_at, now=now, precise=True)}"
        if has_agent(project)
        else "no agent"
    )
    session = project.agent.session_id
    commit = (
        f"commit {format_silence(project.git.last_commit_at, now=now)} ago"
        if project.git.last_commit_at
        else ""
    )
    return [
        _split(f" {project.name}", f"{branch} {dirty} · {silence}".strip(), width),
        _split(
            f" {_tilde(project.path, home)}",
            f"sess {session[:18]}" if session else "",
            width,
        ),
        _split(f" {_agent_label(project)}  {_event_label(project)}".rstrip(), commit, width),
    ]


def render_browser(
    radar: Radar,
    *,
    now: datetime,
    width: int = 78,
    height: int = 40,
    selected: Project | None = None,
    query: str = "",
    layout: str | None = None,
    home: str | None = None,
) -> list[str]:
    """Render the driveable project list, with a detail pane for ``selected``.

    ``layout`` is ``"right"`` or ``"bottom"``; when omitted it comes from
    :func:`~petridish.tui_state.detail_layout`, which derives it from ``width``
    so a terminal resize needs no extra handling.
    """
    from petridish.tui_state import detail_layout

    if layout is None:
        layout = detail_layout(width)

    visible = filter_projects(_visible(radar), query)
    buckets = browser_groups(radar, query)

    if layout == "right":
        left_w = max(20, min(52, width - 46))
        list_lines = _browser_list(
            buckets, now=now, width=left_w, selected=selected
        )
        detail = (
            format_detail(selected)
            if selected is not None
            else ["nothing selected"]
        )
        title = selected.name if selected is not None else ""
        return _join_columns(
            radar,
            now=now,
            width=width,
            height=height,
            left_w=left_w,
            list_lines=list_lines,
            detail=[title, *detail] if title else detail,
            query=query,
            n_projects=len(visible),
        )

    lines = _header(
        radar, now=now, width=width, title="browser", n_projects=len(visible)
    )
    detail_lines = (
        format_detail_compact(selected, now=now, width=width, home=home)
        if selected is not None
        else []
    )
    # Reserve the pane before laying out the list, so the two never fight over
    # the same rows — the bug the old render loop had.
    reserved = (len(detail_lines) + 2) if detail_lines else 0
    footer = 2
    budget = max(0, height - len(lines) - reserved - footer)

    body = _browser_list(buckets, now=now, width=width, selected=selected)
    lines += body[:budget]

    if detail_lines:
        lines.append("─" * width)
        lines += detail_lines
    lines.append("─" * width)
    lines.append(BROWSER_KEYS)
    return _clip(lines, width=width, height=height)


#: Column index dropped first when the browser table will not fit, then the
#: next, and so on. ``agent`` goes first because the detail pane already names
#: it; ``branch`` second. ``name``, ``✎`` and ``silent`` are never dropped —
#: silence is the reason the screen exists.
_DROPPABLE_COLS = (1, 2)


def _budget_columns(
    rows: list[list[str]], width: int
) -> tuple[list[str], list[list[str]], list[int]]:
    """Fit the browser table into ``width`` by dropping columns, not truncating.

    Blind truncation was the first implementation and it was wrong: at 100
    columns the right-hand pane left only ~52 for the table, and the ellipsis
    ate the *silence* column — the one value the screen exists to show. Dropping
    a whole low-value column keeps the survivors readable instead.

    Returns ``(headers, rows, widths)`` ready for :func:`pad_row`.
    """
    headers = list(BROWSER_HEADERS)
    rows = [list(r) for r in rows]
    #: 2 leading columns for the selection marker, 2 per gutter.
    overhead = 2

    def total(ws: list[int]) -> int:
        return sum(ws) + 2 * max(0, len(ws) - 1) + overhead

    widths = column_widths(rows, headers)
    for col in _DROPPABLE_COLS:
        if total(widths) <= width:
            break
        # Recompute the live index each pass: dropping column 1 shifts what was
        # column 2 down to 1.
        idx = col - _DROPPABLE_COLS.index(col)
        if idx >= len(headers):
            continue
        del headers[idx]
        for row in rows:
            del row[idx]
        widths = column_widths(rows, headers)

    # Every droppable column is gone and it still does not fit: give the excess
    # back from the name column, which is the only one with slack left.
    excess = total(widths) - width
    if excess > 0 and widths:
        widths[0] = max(6, widths[0] - excess)
        rows = [[_fit(c, widths[i]) for i, c in enumerate(r)] for r in rows]
        headers = [_fit(h, widths[i]) for i, h in enumerate(headers)]

    return headers, rows, widths


def _browser_list(
    buckets: dict[str, list[Project]],
    *,
    now: datetime,
    width: int,
    selected: Project | None,
) -> list[str]:
    """Bucket-sectioned project table, one line per project.

    Column widths are computed across *every* visible row rather than per
    section, so the columns line up across bucket boundaries the way
    ``swab list``'s do.
    """
    ordered = [(b, ps) for b, ps in buckets.items() if ps]
    all_rows = [format_browser_row(p, now=now) for _b, ps in ordered for p in ps]
    if not all_rows:
        return [" no projects match"]

    headers, fitted, widths = _budget_columns(all_rows, width)
    lines = [_fit("  " + pad_row(headers, widths), width)]

    cursor = 0
    for bucket, projects in ordered:
        lines.append(
            _split(f" {_BUCKET_LABELS[bucket]}", str(len(projects)), width).rstrip()
        )
        for project in projects:
            row = pad_row(fitted[cursor], widths)
            cursor += 1
            marker = ">" if selected is not None and project.id == selected.id else " "
            lines.append(_fit(f"{marker} {row}", width).rstrip())
    return lines


def _join_columns(
    radar: Radar,
    *,
    now: datetime,
    width: int,
    height: int,
    left_w: int,
    list_lines: list[str],
    detail: list[str],
    query: str,
    n_projects: int,
) -> list[str]:
    """Stitch the list and the detail pane into a two-column screen.

    The vertical rule sits at a fixed column, which is why :func:`_split` pads
    to exactly ``width`` — a ragged left column would make the rule wander.
    """
    lines = _header(
        radar, now=now, width=width, title="browser", n_projects=n_projects
    )
    # Re-cut the header's heavy rule so the column divider starts on it. Index
    # -1, not 1: the header grew a quota line between the title and the rule,
    # and a hardcoded index silently overwrote it here.
    lines[-1] = "═" * left_w + "╤" + "═" * max(0, width - left_w - 1)

    body_h = max(0, height - len(lines) - 2)
    for i in range(body_h):
        left = list_lines[i] if i < len(list_lines) else ""
        right = detail[i] if i < len(detail) else ""
        lines.append(f"{_fit(left, left_w).ljust(left_w)}│ {_fit(right, max(0, width - left_w - 2))}".rstrip())

    lines.append("─" * left_w + "┴" + "─" * max(0, width - left_w - 1))
    lines.append(f" /{query}" if query else BROWSER_KEYS)
    return _clip(lines, width=width, height=height)


__all__ = [
    "render_dashboard",
    "format_quota_line",
    "QUOTA_BAR_CELLS",
    "QUOTA_STALE_AFTER_S",
    "browser_groups",
    "DASHBOARD_KEYS",
    "BROWSER_KEYS",
    "render_browser",
    "format_card",
    "format_compact_row",
    "format_browser_row",
    "format_detail_compact",
    "ROOMY_CARD_HEIGHT",
    "BROWSER_HEADERS",
]
