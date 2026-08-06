"""Read and compact agent-event ndjson into one ``AgentSignal`` per project root."""

import json
import os
from datetime import datetime
from pathlib import Path
from typing import Union

from radar.discovery import resolve_root
from radar.schema import AgentSignal


def read_and_compact(
    path: Union[str, Path],
    config,
    *,
    max_bytes: int = 5_242_880,
) -> dict[str, AgentSignal]:
    """Read ndjson events from *path*, fold to one ``AgentSignal`` per root (newest
    ``at`` wins), then truncate the file so events are consumed exactly once.

    Never raises — a missing or unreadable file returns ``{}``. Malformed or
    partial lines are silently skipped, per CLAUDE.md rule #4 (trailing truncated
    JSONL lines are normal because live sessions keep being appended to while we
    read).

    ``max_bytes`` is a soft cap: when the file has grown large (e.g. the daemon
    has been down for a while), reading past it is aborted so we never hang on
    a multi-gigabyte backlog. The file is still truncated at the end so events
    beyond the cap are dropped on this read and never come back.
    """
    try:
        path_str = str(path)
        with open(path_str, "r", encoding="utf-8") as f:
            content = f.read()
    except (OSError, ValueError):
        return {}

    signals: dict[str, AgentSignal] = {}
    bytes_seen = 0

    for raw_line in content.splitlines():
        stripped = raw_line.strip()
        if not stripped:
            continue

        line_bytes = len(stripped.encode("utf-8"))
        if bytes_seen > 0 and bytes_seen + line_bytes > max_bytes:
            # Soft cap — stop reading past the budget. Anything already in
            # ``signals`` was processed; anything beyond is dropped on truncate.
            break
        bytes_seen += line_bytes

        try:
            record = json.loads(stripped)
        except (json.JSONDecodeError, ValueError):
            continue
        if not isinstance(record, dict):
            continue

        try:
            cwd = record["cwd"]
            session_id = record["session_id"]
            event_name = record["event"]
            at_str = record["at"]
        except KeyError:
            continue

        root = resolve_root(Path(cwd), config)
        at = _parse_iso_at(at_str)
        signal = AgentSignal(
            root=str(root),
            at=at,
            agent="claude-code",
            session_id=session_id,
            event=event_name,
            raw_cwd=cwd,
        )

        key = str(root)
        existing = signals.get(key)
        if existing is None or at > existing.at:
            signals[key] = signal

    # Truncate — events consumed exactly once.
    try:
        with open(path_str, "w") as f:
            pass
    except OSError:
        pass

    return signals


def _parse_iso_at(s: str) -> datetime:
    """Parse an ISO-8601 timestamp, tolerating a trailing ``Z``.

    ``datetime.fromisoformat`` accepts trailing ``Z`` on 3.11+, but we keep the
    fallback so callers can hand us either form without thinking about it.
    """
    if s.endswith("Z"):
        s = s[:-1] + "+00:00"
    return datetime.fromisoformat(s)
