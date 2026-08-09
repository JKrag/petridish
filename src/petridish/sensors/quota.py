"""Read Claude subscription quota from Claude Code's own status file.

Claude Code writes ``~/.claude/last-status.json`` as it runs, and it carries
exactly what a dashboard header wants::

    {"ts": "2026-08-09T06:32:11Z",
     "rate_limits": {
       "five_hour": {"used_percentage": 9,  "resets_at": 1786275000},
       "seven_day": {"used_percentage": 86, "resets_at": 1786431600}},
     "context_window": {..., "used_percentage": 28}}

It updates per message, not per session — the five-hour figure was observed
moving 0% -> 9% between two reads a minute apart.

**This is an undocumented internal file of another program.** It is not a
published API; the shape can change or the file can vanish on any Claude Code
upgrade. So every function here returns ``None`` rather than raising, and no
field is required: a partial read yields a partial :class:`QuotaState` rather
than nothing. That is invariant 5 (sensors degrade, never abort) applied to a
source we do not control.

Two consequences for anything rendering this:

* The numbers are **account-global**, not per-project. They belong in a header,
  never in a project row.
* The file only changes while a Claude Code session is running, so overnight it
  goes stale while the rest of ``projects.json`` stays fresh. ``measured_at``
  carries Claude Code's own timestamp so a frontend can say how old it is.
"""

from __future__ import annotations

import json
import os
from datetime import datetime, timezone
from typing import Any, cast

from petridish.schema import QuotaState

#: Where Claude Code keeps it. Relative to ``$HOME`` so tests can redirect it.
STATUS_FILE = os.path.join(".claude", "last-status.json")

#: Beyond this, a reported ``resets_at`` is treated as garbage rather than
#: rendered. Both windows are at most 7 days, so a reset more than 30 days out
#: means we misread the units (seconds vs milliseconds, say) and showing
#: "resets in 20000d" would be worse than showing nothing.
_MAX_RESET_HORIZON_S = 30 * 24 * 3600


def _as_dict(value: object) -> dict[str, Any]:
    """``value`` if it is a mapping, else an empty one.

    Every level of this payload belongs to another program and may stop being a
    dict at any version. Funnelling each access through here means a shape
    change degrades to "field absent" instead of raising, and it keeps the
    ``isinstance`` narrowing (which strict pyright resolves to
    ``dict[Unknown, Unknown]``) in exactly one place.
    """
    return cast("dict[str, Any]", value) if isinstance(value, dict) else {}


def _epoch_to_dt(value: object, *, now: datetime) -> datetime | None:
    """Convert an epoch-seconds number to UTC, or ``None`` if implausible.

    ``bool`` is rejected explicitly: it is an ``int`` subclass, so ``True``
    would otherwise become 1970-01-01.

    That guard is *deliberately redundant* today — 1970 is 56 years outside
    :data:`_MAX_RESET_HORIZON_S`, so the horizon check below already rejects it,
    and mutation testing correctly reports removing it as an equivalent mutant.
    It stays because it is independent of the horizon constant: widen that and
    the bool case would start slipping through. Don't delete it as dead code.
    """
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    try:
        dt = datetime.fromtimestamp(float(value), timezone.utc)
    except (OverflowError, OSError, ValueError):
        return None
    if abs((dt - now).total_seconds()) > _MAX_RESET_HORIZON_S:
        return None
    return dt.replace(microsecond=0)


def _pct(value: object) -> int | None:
    """A 0-100 integer, or ``None``. Out-of-range values are dropped."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    n = int(value)
    return n if 0 <= n <= 100 else None


def _parse_ts(value: object, *, now: datetime) -> datetime | None:
    """Claude Code's own ISO-8601 ``ts``, or ``None``."""
    if not isinstance(value, str):
        return None
    try:
        dt = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    dt = dt.astimezone(timezone.utc).replace(microsecond=0)
    if abs((dt - now).total_seconds()) > _MAX_RESET_HORIZON_S:
        return None
    return dt


def parse_status(payload: object, *, now: datetime) -> QuotaState | None:
    """Build a :class:`QuotaState` from a decoded ``last-status.json``.

    Split out from the file read so the parsing rules can be tested against
    hand-built payloads — including the malformed ones a schema change would
    produce — without touching the filesystem.

    Returns ``None`` only when the payload is not a mapping or carries no
    usable field at all. A payload with *some* recognisable fields yields a
    :class:`QuotaState` with the rest left ``None``; partial truth beats none.
    """
    # No ``isinstance`` guard: a non-mapping funnels through ``_as_dict`` to an
    # empty one, every field comes back ``None``, and the "nothing recognisable"
    # check below returns ``None`` anyway. One exit for both cases.
    data = _as_dict(payload)

    limits = _as_dict(data.get("rate_limits"))
    five = _as_dict(limits.get("five_hour"))
    seven = _as_dict(limits.get("seven_day"))
    context = _as_dict(data.get("context_window"))

    state = QuotaState(
        measured_at=_parse_ts(data.get("ts"), now=now),
        five_hour_used_pct=_pct(five.get("used_percentage")),
        five_hour_resets_at=_epoch_to_dt(five.get("resets_at"), now=now),
        seven_day_used_pct=_pct(seven.get("used_percentage")),
        seven_day_resets_at=_epoch_to_dt(seven.get("resets_at"), now=now),
        context_used_pct=_pct(context.get("used_percentage")),
    )
    if state == QuotaState():
        # Nothing recognisable. Report absence rather than a row of blanks,
        # so the header can omit the line entirely.
        return None
    return state


def read_quota(
    home: str | None = None, *, now: datetime | None = None
) -> QuotaState | None:
    """Read and parse Claude Code's status file. Never raises.

    ``home`` defaults to ``$HOME``; tests pass a temporary directory. Returns
    ``None`` when the file is missing, unreadable, not JSON, or carries nothing
    we recognise — all of which are ordinary, not errors.
    """
    if now is None:
        now = datetime.now(timezone.utc)
    base = home if home is not None else os.path.expanduser("~")
    path = os.path.join(base, STATUS_FILE)
    try:
        with open(path, "r", encoding="utf-8") as fh:
            payload = json.load(fh)
    except (OSError, ValueError):
        # Missing, unreadable, mid-write, or not JSON. All ordinary.
        return None
    try:
        return parse_status(payload, now=now)
    except Exception:
        # The parser is defensive field-by-field, but this file belongs to
        # another program: a shape we have not imagined must not kill the tick.
        return None


__all__ = ["read_quota", "parse_status", "STATUS_FILE"]
