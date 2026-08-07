"""petridish scan orchestration.

Merges discovery results and agent-sensor observations into a single Radar
snapshot, applying the bucketing rules described in IMPLEMENTATION_PLAN.md.
"""

from __future__ import annotations

import hashlib
import os
import time
from pathlib import Path
from datetime import datetime
from typing import Optional

from petridish.config import Config
from petridish.discovery import discover as _discover, is_foreign, resolve_root
from petridish.events import read_and_compact as _read_events
from petridish.git import scan as _git_scan
from petridish.sensors.claude import scan as claude_scan
from petridish.sensors.copilot import scan as copilot_scan
from petridish.schema import (
    AgentSignal,
    AgentState,
    GitState,
    Project,
    Radar,
    write_atomic,
    utcnow,
    to_utc,
)


def _sha1_id(resolved_path: str) -> str:
    return hashlib.sha1(str(resolved_path).encode("utf-8")).hexdigest()[:12]


def _agent_state_for(signal: Optional[AgentSignal], now: datetime) -> AgentState:
    if signal is None:
        return AgentState(
            state="idle",
            active_agent=None,
            last_event=None,
            last_event_at=None,
            session_id=None,
        )
    now_t = to_utc(now)
    age_s = (now_t - to_utc(signal.at)).total_seconds()
    if age_s < 90:
        state = "working"
    elif age_s < 30 * 60:
        state = "recent"
    else:
        state = "idle"
    # An old signal still carries useful facts: WHICH agent last touched this
    # project, WHEN, and the session id needed to resume it
    # (`claude --resume <session_id>`). Only ``state`` reflects recency —
    # blanking the metadata for idle projects would strip the resume feature
    # from exactly the projects you most want to resume.
    return AgentState(
        state=state,
        active_agent=signal.agent,
        last_event=signal.event,
        last_event_at=signal.at,
        session_id=signal.session_id,
    )


def _last_activity(
    git_state: GitState, signal: Optional[AgentSignal]
) -> Optional[datetime]:
    """Newest non-None of mine_last_commit_at, last_commit_at, signal.at.

    When both git dates exist we prefer mine_last_commit_at (a teammate's
    push is not your activity) but still fall back to last_commit_at when
    there is no authored commit.
    """
    try:
        mine = git_state.mine_last_commit_at  # type: ignore[attr-defined]
    except Exception:
        mine = None

    try:
        last = git_state.last_commit_at  # type: ignore[attr-defined]
    except Exception:
        last = None

    sig_at = signal.at if signal is not None else None

    # Normalise to tz-AWARE UTC so offsets don't break max(). Returning a naive
    # datetime here would violate the schema's tz-aware contract and make an
    # in-memory Radar unequal to the one read back off disk.
    def _norm(dt) -> datetime | None:
        return None if dt is None else to_utc(dt)

    if mine is not None:
        candidates = [_norm(mine)]
    elif last is not None:
        candidates = [_norm(last)]
    else:
        candidates = []

    candidates.append(_norm(sig_at))  # _norm handles None

    non_none = [c for c in candidates if c is not None]
    return max(non_none) if non_none else None


def _bucket(
    last_activity_at: Optional[datetime],
    agent_state: AgentState,
    now: datetime,
    thresholds: dict,
) -> str:
    if agent_state.state in ("working", "recent"):
        return "active"
    if last_activity_at is None:
        return "cold"
    # Normalize to naive UTC so offsets from git's author_date don't break us.
    now_t = to_utc(now)
    age_h = (now_t - to_utc(last_activity_at)).total_seconds() / 3600.0
    active_h = thresholds.get("active", 48.0)
    in_flight_h = thresholds.get("in_flight", 336.0)
    stale_h = thresholds.get("stale", 1440.0)
    if age_h < active_h:
        return "active"
    if age_h < in_flight_h:
        return "in_flight"
    if age_h < stale_h:
        return "stale"
    return "cold"


def _build_project(
    root: str, config: Config, signal: Optional[AgentSignal], now: datetime
) -> Project:
    abs_path = os.path.abspath(root)
    # ``resolve_root`` requires ``config``. The old fallback here called it with
    # one argument inside ``except TypeError``, so it could only ever raise a
    # second TypeError from the arity error — masking whatever real TypeError
    # came out of the function body. Degrade to the literal path instead: a
    # monorepo subdir that fails to collapse to its parent is wrong, but far
    # better than losing the whole tick (invariant 5).
    #
    # Keep ``resolved`` a ``str``. ``resolve_root`` returns a ``Path``, and
    # ``category_overrides`` is a ``dict[str, str]`` keyed by path strings — a
    # ``Path`` key hashes differently, so the lookup below silently missed
    # *every* time and the whole category-override feature was inert.
    try:
        resolved = str(resolve_root(abs_path, config))
    except Exception:
        resolved = abs_path

    parent_name = os.path.basename(os.path.dirname(resolved))
    category = config.category_overrides.get(resolved, parent_name)

    try:
        foreign = is_foreign(resolved, config)
    except Exception:
        foreign = False

    try:
        git_state = _git_scan(resolved, config.author_patterns, config.author_since)
    except Exception:
        git_state = GitState(is_repo=False)

    agent = _agent_state_for(signal, now)
    last_activity_at = _last_activity(git_state, signal)
    status_bucket = _bucket(
        last_activity_at, agent, now, config.bucket_thresholds
    )

    return Project(
        id=_sha1_id(resolved),
        name=os.path.basename(resolved),
        # Schema declares path: str; ``resolved`` is already normalised above.
        path=resolved,
        category=category,
        is_foreign=foreign,
        git=git_state,
        agent=agent,
        last_activity_at=last_activity_at,
        status_bucket=status_bucket,
    )


def run_scan(
    config: Config,
    *,
    claude_dir: Optional[str] = None,
    copilot_dir: Optional[str] = None,
    events_path: Optional[str] = None,
    now: Optional[datetime] = None,
) -> Radar:
    """Build a Radar snapshot from discovery + sensor fusion."""
    if now is None:
        now = utcnow()
    # Normalise at the boundary: the schema treats a naive datetime as UTC and
    # truncates to second resolution when serialising, so storing the caller's
    # raw value would make the in-memory Radar unequal to the one read back
    # off disk.
    now = to_utc(now)

    t0 = time.monotonic()

    # Fill in the real locations when the caller didn't inject fixtures.
    # These were declared but never defaulted, so in production every sensor
    # got None, raised, and was swallowed by the except blocks below into an
    # empty dict — a projects.json with zero agent activity that looked
    # perfectly healthy. The tests never caught it because they all either
    # monkeypatch the sensors or pass explicit fixture directories.
    if claude_dir is None:
        claude_dir = Path.home() / ".claude" / "projects"
    if copilot_dir is None:
        copilot_dir = (
            Path.home() / "Library" / "Application Support" / "Code"
            / "User" / "workspaceStorage"
        )
    if events_path is None:
        events_path = Path.home() / ".petridish" / "events.ndjson"

    paths = _discover(config)

    try:
        claude_signals = claude_scan(claude_dir, config)
    except Exception:
        claude_signals = {}

    try:
        copilot_signals = copilot_scan(copilot_dir, config)
    except Exception:
        copilot_signals = {}

    try:
        hook_signals = _read_events(events_path, config)
    except Exception:
        hook_signals = {}

    # Step 4: merge by root, newest at wins.
    merged: dict[str, AgentSignal] = {}
    for src in (claude_signals, copilot_signals, hook_signals):
        for root, sig in src.items():
            existing = merged.get(root)
            if existing is None or sig.at > existing.at:
                merged[root] = sig

    # Step 5/6: every signal root counts; every discovered path too.
    # Normalize to absolute-path strings so Path and str keys merge cleanly.
    seen_roots: set[str] = set()

    def _add(root: str | os.PathLike) -> None:
        key = os.path.realpath(os.fspath(root))
        seen_roots.add(key)

    for p in paths:
        _add(p)
    for root in merged.keys():
        _add(root)

    projects = [
        _build_project(r, config, merged.get(r), now)
        for r in seen_roots
    ]

    # Step 10: last_activity_at desc (None last), then name asc.
    def sort_key(p: Project) -> tuple[int, float, str]:
        if p.last_activity_at is None:
            return (1, 0.0, p.name)
        return (0, -(p.last_activity_at.timestamp()), p.name)

    projects.sort(key=sort_key)

    elapsed_ms = int((time.monotonic() - t0) * 1000)

    return Radar(
        updated_at=now,
        projects=tuple(projects),
        scan_duration_ms=elapsed_ms,
    )


def write_scan(config: Config, out_path: str, **kwargs) -> Radar:
    radar = run_scan(config, **kwargs)
    write_atomic(radar, out_path)
    return radar
