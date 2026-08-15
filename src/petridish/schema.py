"""The frozen state schema for ``~/.petridish/projects.json``.

This module is the shared contract: the daemon writes it, every frontend reads
it, and each sensor produces :class:`AgentSignal` values that the aggregator
folds into :class:`Project` records.  See ``IMPLEMENTATION_PLAN.md`` §4.

Two rules matter more than the rest:

* **Timestamps are second-resolution UTC**, serialised as ``...Z`` strings.
  Sub-second precision is deliberately discarded — nothing in this system
  reasons about milliseconds, and truncating keeps round-tripping exact.
* **Writes are atomic** (see :func:`write_atomic`).  The daemon is the sole
  writer; readers therefore never observe a partial file and need no lock.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass, field, replace
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Mapping, TypedDict

SCHEMA_VERSION = 1

#: `~/.petridish/` — the daemon's config/state directory. Shared across every
#: frontend (the `petri` TUI, `menubar.py`, `installer.py`) and the Rust
#: scanner (`swab-rs/`), which resolves the identical path independently on
#: its own side (`config::default_path()`).
CONFIG_DIR = os.path.join(os.path.expanduser("~"), ".petridish")

#: Default `projects.json` state path — the file the Rust scanner
#: (`swab scan`) writes and every Python frontend reads via `read_json`.
_DEFAULT_STATE_PATH = os.path.join(CONFIG_DIR, "projects.json")

#: Structural marker tagging every hook entry `installer.py` writes into
#: `~/.claude/settings.json`, so `--uninstall` can remove exactly (and only)
#: what this project added without disturbing other hook consumers.
HOOK_MARKER = "# petridish"

# ---------------------------------------------------------------------------
# Serialised shapes.
#
# These TypedDicts are the wire contract for ``~/.petridish/projects.json`` —
# the same shape Raycast's `raycast/src/types.ts` mirrors by hand. Declaring
# them here makes that contract machine-checked: add a field to a dataclass and
# forget it in ``to_dict``, and pyright fails instead of a frontend silently
# reading ``undefined``.
#
# Note the deliberate asymmetry with ``from_dict``. ``to_dict`` returns a
# TypedDict because *we* produce that shape and guarantee it. ``from_dict``
# takes a permissive ``Mapping[str, Any]`` because it parses whatever is
# actually on disk — a hand-edited, truncated, or older-version file is
# expected input, not a type error. Typing the input strictly would be a lie
# that pushes real parsing failures to runtime.
# ---------------------------------------------------------------------------


class GitStateDict(TypedDict):
    is_repo: bool
    branch: str | None
    is_dirty: bool
    uncommitted_files: int
    last_commit_at: str | None
    mine_last_commit_at: str | None
    github_url: str | None


class AgentStateDict(TypedDict):
    state: str
    active_agent: str | None
    last_event: str | None
    last_event_at: str | None
    session_id: str | None


# Allowed enum-ish values. Kept as plain tuples rather than Enums so the JSON
# stays trivially readable from TypeScript/jq without a mapping layer.
AGENT_STATES = ("working", "recent", "idle")
STATUS_BUCKETS = ("active", "in_flight", "stale", "cold")

#: Silence thresholds, in seconds, that define :data:`AGENT_STATES`.
#:
#: These live here rather than in ``scan.py`` because two clocks read them. The
#: daemon stamps ``agent.state`` at *scan* time, but a frontend showing a live
#: "silent 4m 12s" counter must re-derive the state at *render* time — a
#: ``projects.json`` that is 90 seconds old already has a stale ``state`` field.
#: With the numbers duplicated, the traffic light and the clock beside it would
#: eventually disagree.
AGENT_WORKING_MAX_S = 90
AGENT_RECENT_MAX_S = 30 * 60


def agent_state_for_silence(silence_s: float) -> str:
    """Map seconds-since-last-event onto one of :data:`AGENT_STATES`.

    The sole definition of what the three states *mean*. Negative input (a
    clock skew between the writer and the reader) is treated as zero rather
    than raising — a frontend must never crash on a timestamp from the future.
    """
    if silence_s < 0:
        silence_s = 0.0
    if silence_s < AGENT_WORKING_MAX_S:
        return "working"
    if silence_s < AGENT_RECENT_MAX_S:
        return "recent"
    return "idle"


# ---------------------------------------------------------------------------
# datetime helpers
# ---------------------------------------------------------------------------

def to_utc(dt: datetime) -> datetime:
    """Normalise to tz-aware UTC at second resolution.

    A naive datetime is *assumed* to be UTC rather than local time — the
    daemon only ever deals in UTC, and guessing local time here would silently
    shift every timestamp by the machine's offset.
    """
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.astimezone(timezone.utc).replace(microsecond=0)


def utcnow() -> datetime:
    """Current time, tz-aware UTC, second resolution."""
    return datetime.now(timezone.utc).replace(microsecond=0)


def _iso(dt: datetime | None) -> str | None:
    """Serialise to ``2026-08-05T22:45:00Z``; ``None`` passes through."""
    if dt is None:
        return None
    return to_utc(dt).strftime("%Y-%m-%dT%H:%M:%SZ")


def _parse(s: str | None) -> datetime | None:
    """Inverse of :func:`_iso`. Returns a tz-aware UTC datetime."""
    if s is None:
        return None
    # fromisoformat handles "+00:00" natively; normalise the "Z" suffix first.
    return to_utc(datetime.fromisoformat(s.replace("Z", "+00:00")))


# ---------------------------------------------------------------------------
# Records
# ---------------------------------------------------------------------------

@dataclass(frozen=True)
class GitState:
    """Git facts for one project. ``is_repo=False`` is the safe degraded value."""

    is_repo: bool = False
    branch: str | None = None
    is_dirty: bool = False
    uncommitted_files: int = 0
    last_commit_at: datetime | None = None
    #: Last commit authored by *this user* (see the authorship filter, plan §2).
    #: Preferred over ``last_commit_at`` when bucketing, so a teammate's or a
    #: bot's push doesn't make a project look like yours.
    mine_last_commit_at: datetime | None = None
    github_url: str | None = None

    def to_dict(self) -> GitStateDict:
        return {
            "is_repo": self.is_repo,
            "branch": self.branch,
            "is_dirty": self.is_dirty,
            "uncommitted_files": self.uncommitted_files,
            "last_commit_at": _iso(self.last_commit_at),
            "mine_last_commit_at": _iso(self.mine_last_commit_at),
            "github_url": self.github_url,
        }

    @classmethod
    def from_dict(cls, d: Mapping[str, Any]) -> GitState:
        return cls(
            is_repo=d.get("is_repo", False),
            branch=d.get("branch"),
            is_dirty=d.get("is_dirty", False),
            uncommitted_files=d.get("uncommitted_files", 0),
            last_commit_at=_parse(d.get("last_commit_at")),
            mine_last_commit_at=_parse(d.get("mine_last_commit_at")),
            github_url=d.get("github_url"),
        )


@dataclass(frozen=True)
class AgentState:
    """Live agent activity for one project, as published to frontends."""

    state: str = "idle"
    active_agent: str | None = None
    last_event: str | None = None
    last_event_at: datetime | None = None
    session_id: str | None = None

    def to_dict(self) -> AgentStateDict:
        return {
            "state": self.state,
            "active_agent": self.active_agent,
            "last_event": self.last_event,
            "last_event_at": _iso(self.last_event_at),
            "session_id": self.session_id,
        }

    @classmethod
    def from_dict(cls, d: Mapping[str, Any]) -> AgentState:
        return cls(
            state=d.get("state", "idle"),
            active_agent=d.get("active_agent"),
            last_event=d.get("last_event"),
            last_event_at=_parse(d.get("last_event_at")),
            session_id=d.get("session_id"),
        )


@dataclass(frozen=True)
class AgentSignal:
    """One sensor's observation. **Internal — never serialised into projects.json.**

    Every sensor (M4 Claude, M5 Copilot, M6 hook events) returns
    ``dict[str, AgentSignal]`` keyed by ``root``, already collapsed to the
    newest signal per root.  The aggregator merges across sensors by the same
    newest-wins rule.

    ``root`` is a *resolved project root* — the sensor has already walked it up
    from the raw cwd via ``discovery.resolve_root`` — so that a monorepo session
    in ``repo/packages/core`` attributes to ``repo`` rather than fragmenting
    into phantom projects.  ``raw_cwd`` keeps the pre-resolution value for
    debugging that attribution.
    """

    root: str
    at: datetime
    agent: str
    session_id: str | None = None
    event: str | None = None
    raw_cwd: str | None = None


class QuotaStateDict(TypedDict):
    measured_at: str | None
    five_hour_used_pct: int | None
    five_hour_resets_at: str | None
    seven_day_used_pct: int | None
    seven_day_resets_at: str | None
    context_used_pct: int | None


@dataclass(frozen=True)
class QuotaState:
    """Claude subscription usage, as last reported by Claude Code itself.

    Sourced from ``~/.claude/last-status.json`` — see `swab-rs`'s
    ``sensors::quota`` module. Account-global, not per-project: this
    belongs in a header, never in a project row.

    Every field is optional and defaults to ``None``. That is not defensive
    padding: the file is an **undocumented internal** of another program, so
    any field may disappear on a Claude Code upgrade, and a missing value must
    degrade to "unknown" rather than break the tick (invariant 5).

    ``measured_at`` is when *Claude Code* wrote the numbers, not when the
    daemon read them. A frontend needs it to say how stale the figures are:
    the file only updates while a session is running, so overnight it can be
    hours old while everything else in ``projects.json`` is a minute old.
    """

    measured_at: datetime | None = None
    five_hour_used_pct: int | None = None
    five_hour_resets_at: datetime | None = None
    seven_day_used_pct: int | None = None
    seven_day_resets_at: datetime | None = None
    context_used_pct: int | None = None

    def to_dict(self) -> QuotaStateDict:
        return {
            "measured_at": _iso(self.measured_at),
            "five_hour_used_pct": self.five_hour_used_pct,
            "five_hour_resets_at": _iso(self.five_hour_resets_at),
            "seven_day_used_pct": self.seven_day_used_pct,
            "seven_day_resets_at": _iso(self.seven_day_resets_at),
            "context_used_pct": self.context_used_pct,
        }

    @classmethod
    def from_dict(cls, d: Mapping[str, Any]) -> QuotaState:
        return cls(
            measured_at=_parse(d.get("measured_at")),
            five_hour_used_pct=d.get("five_hour_used_pct"),
            five_hour_resets_at=_parse(d.get("five_hour_resets_at")),
            seven_day_used_pct=d.get("seven_day_used_pct"),
            seven_day_resets_at=_parse(d.get("seven_day_resets_at")),
            context_used_pct=d.get("context_used_pct"),
        )


class ProjectDict(TypedDict):
    id: str
    name: str
    path: str
    category: str
    is_foreign: bool
    git: GitStateDict
    agent: AgentStateDict
    last_activity_at: str | None
    status_bucket: str


@dataclass(frozen=True)
class Project:
    """One tracked project."""

    id: str
    name: str
    path: str
    category: str
    is_foreign: bool = False
    git: GitState = field(default_factory=GitState)
    agent: AgentState = field(default_factory=AgentState)
    last_activity_at: datetime | None = None
    status_bucket: str = "cold"

    def to_dict(self) -> ProjectDict:
        return {
            "id": self.id,
            "name": self.name,
            "path": self.path,
            "category": self.category,
            "is_foreign": self.is_foreign,
            "git": self.git.to_dict(),
            "agent": self.agent.to_dict(),
            "last_activity_at": _iso(self.last_activity_at),
            "status_bucket": self.status_bucket,
        }

    @classmethod
    def from_dict(cls, d: Mapping[str, Any]) -> Project:
        return cls(
            id=d["id"],
            name=d["name"],
            path=d["path"],
            category=d["category"],
            is_foreign=d.get("is_foreign", False),
            git=GitState.from_dict(d.get("git") or {}),
            agent=AgentState.from_dict(d.get("agent") or {}),
            last_activity_at=_parse(d.get("last_activity_at")),
            status_bucket=d.get("status_bucket", "cold"),
        )


class RadarDict(TypedDict):
    schema_version: int
    updated_at: str | None
    scan_duration_ms: int
    projects: list[ProjectDict]
    #: Always present (possibly ``null``) so the shape we *write* is total,
    #: even though what we *read* may predate the field entirely.
    quota: QuotaStateDict | None


@dataclass(frozen=True)
class Radar:
    """The whole ``projects.json`` document."""

    updated_at: datetime
    projects: tuple[Project, ...] = ()
    scan_duration_ms: int = 0
    schema_version: int = SCHEMA_VERSION
    #: Account-wide Claude quota, or ``None`` when the sensor found nothing.
    #: Optional and last so every existing construction site keeps working.
    quota: QuotaState | None = None

    def to_dict(self) -> RadarDict:
        return {
            "schema_version": self.schema_version,
            "updated_at": _iso(self.updated_at),
            "scan_duration_ms": self.scan_duration_ms,
            "projects": [p.to_dict() for p in self.projects],
            "quota": self.quota.to_dict() if self.quota else None,
        }

    @classmethod
    def from_dict(cls, d: Mapping[str, Any]) -> Radar:
        updated = _parse(d.get("updated_at"))
        if updated is None:
            raise ValueError("projects.json is missing required 'updated_at'")
        return cls(
            updated_at=updated,
            # Must be a tuple: a list would break both frozen-ness and the
            # round-trip equality contract.
            projects=tuple(Project.from_dict(p) for p in d.get("projects", ())),
            scan_duration_ms=d.get("scan_duration_ms", 0),
            schema_version=d.get("schema_version", SCHEMA_VERSION),
            # Absent in every file written before this field existed, and null
            # whenever the sensor came up empty. Both mean "unknown", not "0%".
            quota=(
                QuotaState.from_dict(d["quota"])
                if isinstance(d.get("quota"), dict)
                else None
            ),
        )

    def to_json(self, *, indent: int | None = 2) -> str:
        return json.dumps(self.to_dict(), indent=indent) + "\n"

    @classmethod
    def from_json(cls, text: str) -> Radar:
        return cls.from_dict(json.loads(text))


# ---------------------------------------------------------------------------
# Atomic write
# ---------------------------------------------------------------------------

def write_atomic(radar: Radar, path: str | os.PathLike) -> None:
    """Write ``radar`` to ``path`` atomically.

    Serialises to a sibling ``.tmp`` file, then :func:`os.replace` onto the
    target — ``os.replace`` is atomic within a filesystem, so a concurrent
    reader sees either the old file or the new one, never a half-written one.
    The temp file must be a *sibling* (not in /tmp) for that guarantee to hold.

    The parent directory is created if missing.  On any failure the temp file is
    removed rather than left behind for the next run to trip over.
    """
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    tmp = target.with_name(target.name + ".tmp")
    try:
        tmp.write_text(radar.to_json(), encoding="utf-8")
        os.replace(tmp, target)
    except BaseException:
        try:
            tmp.unlink(missing_ok=True)
        except OSError:
            pass
        raise


def read_json(path: str | os.PathLike) -> Radar:
    """Load a :class:`Radar` from disk. Raises ``FileNotFoundError`` if absent."""
    return Radar.from_json(Path(path).read_text(encoding="utf-8"))


__all__ = [
    "SCHEMA_VERSION",
    "AGENT_STATES",
    "STATUS_BUCKETS",
    "AGENT_WORKING_MAX_S",
    "AGENT_RECENT_MAX_S",
    "agent_state_for_silence",
    "GitState",
    "AgentState",
    "QuotaState",
    "AgentSignal",
    "Project",
    "Radar",
    "write_atomic",
    "read_json",
    "to_utc",
    "utcnow",
    "replace",
]
