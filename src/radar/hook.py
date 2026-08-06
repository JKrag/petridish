"""radar-hook console script — the hot path for tool-use events.

Runs on every tool use, alongside three other hook consumers. Must be tiny,
silent, and never raise: a failure here would disrupt all hook consumers.

Imports are kept to stdlib primitives — no heavy radar modules — so the
per-invocation overhead is near zero.
"""

import json
import os
import sys
from datetime import datetime, timezone


def main() -> int:
    """Append one ndjson event line and return 0, unconditionally.

    Reads the hook event JSON from stdin, extracts cwd / session_id and
    hook_event_name, and appends a single JSON line to
    ``~/.project-radar/events.ndjson`` (or the path given by the
    ``RADAR_EVENTS_PATH`` environment variable for tests).
    """
    try:
        events_path = _events_path()

        raw = sys.stdin.read()
        if not raw.strip():
            return 0

        data = json.loads(raw)

        # Only ``cwd`` is required — it IS the signal. session_id and the event
        # name are enrichment, and dropping an otherwise-valid event because one
        # of them is absent would silently lose real activity.
        cwd = data.get("cwd")
        if not cwd:
            return 0
        session_id = data.get("session_id") or data.get("sessionId")
        event_name = data.get("hook_event_name")

        at_str = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        line = json.dumps({
            "cwd": cwd,
            "session_id": session_id,
            "event": event_name,
            "at": at_str,
        }) + "\n"

        parent = os.path.dirname(events_path)
        if parent:
            os.makedirs(parent, exist_ok=True)

        fd = os.open(events_path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
        try:
            os.write(fd, line.encode("utf-8"))
        finally:
            os.close(fd)

        return 0
    except BaseException:
        return 0


def _events_path() -> str:
    """Resolve the target events file path.

    Honours ``RADAR_EVENTS_PATH`` (used by tests); falls back to the user's
    home directory otherwise.
    """
    override = os.environ.get("RADAR_EVENTS_PATH")
    if override:
        return override
    home = os.environ.get("HOME", os.path.expanduser("~"))
    return os.path.join(home, ".project-radar", "events.ndjson")
