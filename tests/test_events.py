"""Tests for ``petridish.hook`` and ``petridish.events``.

Coverage:

* hook.main happy path (one valid event appended, exit code 0)
* hook.main malformed/empty stdin — never raises, always returns 0
* hook accepts both ``session_id`` and ``sessionId`` spellings
* read_and_compact on a missing file returns ``{}``
* read_and_compact skips malformed/truncated lines but keeps valid ones
* read_and_compact folds two events for the same root to the newest ``at``
* read_and_compact truncates the file (second call returns ``{}``)
* Concurrency: 8 parallel processes each append one line; no loss, no interleave
"""

import io
import json
import os
from concurrent.futures import ProcessPoolExecutor
from datetime import datetime, timezone
from pathlib import Path

import pytest

from petridish.events import read_and_compact
from petridish.hook import main as hook_main


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_event(
    cwd: str = "/home/user/project",
    session_id: str = "abc123",
    event_name: str = "tool_use",
) -> dict:
    return {"cwd": cwd, "session_id": session_id, "hook_event_name": event_name}


def _make_line(
    cwd: str = "/home/user/project",
    session_id: str = "s1",
    event: str = "e1",
    at: datetime | None = None,
) -> str:
    """Build one ndjson event line (the kind ``hook.main`` would write)."""
    if at is None:
        at = datetime(2026, 8, 6, 12, 0, 0, tzinfo=timezone.utc)
    return json.dumps({
        "cwd": cwd,
        "session_id": session_id,
        "event": event,
        "at": at.isoformat(),
    })


def _dt(year: int, month: int, day: int, hour: int, minute: int = 0) -> datetime:
    return datetime(year, month, day, hour, minute, tzinfo=timezone.utc)


def _rc(path, config=None, **kwargs):
    """Convenience wrapper around ``read_and_compact``."""
    if config is None:
        from petridish.config import Config

        config = Config()
    return read_and_compact(str(path), config, **kwargs)


def _set_events_file(tmp_path: Path, monkeypatch) -> Path:
    p = tmp_path / "events.ndjson"
    monkeypatch.setenv("PETRIDISH_EVENTS_PATH", str(p))
    return p


# ---------------------------------------------------------------------------
# hook.main tests
# ---------------------------------------------------------------------------

def test_hook_valid_event_appends_one_line(tmp_path, monkeypatch):
    events_file = _set_events_file(tmp_path, monkeypatch)

    monkeypatch.setattr("sys.stdin", io.StringIO(json.dumps(_make_event())))
    ret = hook_main()

    assert ret == 0, f"hook.main returned {ret}, expected 0"

    text = events_file.read_text()
    lines = [l for l in text.splitlines() if l]
    assert len(lines) == 1, f"expected 1 event line, got:\n{text!r}"

    record = json.loads(lines[0])
    assert record["cwd"] == "/home/user/project"
    assert record["session_id"] == "abc123"
    assert record["event"] == "tool_use"
    assert record["at"].endswith("Z")


def test_hook_malformed_stdin_still_returns_zero(tmp_path, monkeypatch):
    events_file = _set_events_file(tmp_path, monkeypatch)

    monkeypatch.setattr("sys.stdin", io.StringIO("{not valid json {{{"))
    ret = hook_main()

    assert ret == 0
    # Hook never opened the file because parsing failed — nothing to write.
    assert not events_file.exists() or events_file.read_text() == ""


def test_hook_empty_stdin_returns_zero(tmp_path, monkeypatch):
    events_file = _set_events_file(tmp_path, monkeypatch)

    monkeypatch.setattr("sys.stdin", io.StringIO(""))
    ret = hook_main()

    assert ret == 0
    # Empty stdin → no file created.
    assert not events_file.exists() or events_file.read_text() == ""


def test_hook_accepts_session_id_and_sessionId(tmp_path, monkeypatch):
    events_file = _set_events_file(tmp_path, monkeypatch)

    # Call 1: snake_case key.
    monkeypatch.setattr("sys.stdin", io.StringIO(json.dumps({
        "cwd": "/a/b", "session_id": "s1", "hook_event_name": "tool_use",
    })))
    assert hook_main() == 0

    # Call 2: camelCase key.
    monkeypatch.setattr("sys.stdin", io.StringIO(json.dumps({
        "cwd": "/c/d", "sessionId": "s2", "hook_event_name": "tool_use",
    })))
    assert hook_main() == 0

    lines = [l for l in events_file.read_text().splitlines() if l]
    assert len(lines) == 2

    records = [json.loads(l) for l in lines]
    # Output is normalised to ``session_id`` regardless of which key arrived.
    assert records[0]["session_id"] == "s1"
    assert records[1]["session_id"] == "s2"


# ---------------------------------------------------------------------------
# read_and_compact tests
# ---------------------------------------------------------------------------

def test_read_and_compact_missing_file_returns_empty(tmp_path):
    result = _rc(tmp_path / "does_not_exist.ndjson")
    assert result == {}


def test_read_and_compact_skips_malformed_keeps_valid(tmp_path):
    """Malformed and truncated lines are skipped; well-formed lines survive."""
    f = tmp_path / "events.ndjson"
    f.write_text("\n".join([
        _make_line("/x", session_id="s1", event="e1", at=_dt(2026, 8, 6, 12)),
        "GARBAGE",
        _make_line("/y", session_id="s2", event="e2", at=_dt(2026, 8, 6, 13)),
        '{"cwd": "/z", "session_id": "s3", "event": "e3", "at": "2026-08-06T14:00:00"',
        _make_line("/w", session_id="s4", event="e4", at=_dt(2026, 8, 6, 15)),
    ]) + "\n")

    result = _rc(f)

    assert len(result) == 3, f"expected 3 signals, got {len(result)}: {list(result.keys())}"
    assert result["/x"].raw_cwd == "/x"
    assert result["/y"].raw_cwd == "/y"
    assert result["/w"].raw_cwd == "/w"


def test_read_and_compact_folds_two_events_same_root_newest_wins(tmp_path):
    f = tmp_path / "events.ndjson"
    f.write_text("\n".join([
        _make_line("/p", session_id="s1", event="old_event", at=_dt(2026, 8, 6, 10)),
        _make_line("/p", session_id="s2", event="new_event", at=_dt(2026, 8, 6, 15)),
    ]) + "\n")

    result = _rc(f)

    assert len(result) == 1
    signal = result["/p"]
    assert signal.event == "new_event"
    assert signal.session_id == "s2"


def test_read_and_compact_truncates_file_after_read(tmp_path):
    f = tmp_path / "events.ndjson"
    f.write_text(_make_line("/p") + "\n")

    result1 = _rc(f)
    assert len(result1) == 1

    # File was truncated — content gone after the read.
    assert f.read_text() == ""

    # Second call on now-empty file returns empty dict.
    result2 = _rc(f)
    assert result2 == {}


# ---------------------------------------------------------------------------
# Concurrency: 8 parallel processes appending one line each.
#
# O_APPEND semantics on POSIX guarantee atomic single-write appends below
# PIPE_BUF (4 KiB) — every line is well under that, so we expect zero loss
# and zero interleaving across 8 simultaneous writers.
# ---------------------------------------------------------------------------

def _concurrent_append(args):
    """Append one ndjson line to *path* under O_APPEND. Runs in a subprocess."""
    idx, path = args
    record = {
        "cwd": "/home/user/project",
        "session_id": f"sess_{idx:03d}",
        "event": "tool_use",
        "at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    }
    line = json.dumps(record) + "\n"

    parent = os.path.dirname(path)
    if parent:
        os.makedirs(parent, exist_ok=True)

    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
    try:
        os.write(fd, line.encode("utf-8"))
    finally:
        os.close(fd)

    return idx


def test_concurrency_no_loss_no_interleaving(tmp_path, monkeypatch):
    path = str(tmp_path / "events.ndjson")

    with ProcessPoolExecutor(max_workers=8) as pool:
        results = list(pool.map(_concurrent_append, [(i, path) for i in range(8)]))

    assert len(results) == 8, f"expected 8 completions, got {len(results)}"

    text = open(path).read()
    lines = [l for l in text.splitlines() if l]
    assert len(lines) == 8, (
        f"expected 8 lines, got {len(lines)} — possible loss or interleaving:\n{text!r}"
    )

    session_ids = set()
    for line in lines:
        try:
            record = json.loads(line)
        except json.JSONDecodeError as exc:
            pytest.fail(f"Interleaved or truncated line: {line!r} — {exc}")
        session_ids.add(record["session_id"])

    expected = {f"sess_{i:03d}" for i in range(8)}
    assert session_ids == expected


# ---------------------------------------------------------------------------
# Hook robustness (added by the orchestrator)
#
# As delegated, main() required BOTH session_id and hook_event_name and
# silently dropped the event when either was absent — discarding a valid cwd,
# which is the only field that actually matters.
# ---------------------------------------------------------------------------


def _run_hook(monkeypatch, tmp_path, payload):
    import io
    import json as _json
    from petridish import hook

    target = tmp_path / "events.ndjson"
    monkeypatch.setenv("PETRIDISH_EVENTS_PATH", str(target))
    monkeypatch.setattr("sys.stdin", io.StringIO(_json.dumps(payload)))
    rc = hook.main()
    lines = target.read_text().splitlines() if target.exists() else []
    return rc, [_json.loads(x) for x in lines]


def test_hook_records_event_without_session_id(monkeypatch, tmp_path):
    """A missing session_id must not throw away a valid cwd."""
    rc, rows = _run_hook(monkeypatch, tmp_path,
                         {"cwd": "/repo", "hook_event_name": "Stop"})
    assert rc == 0
    assert len(rows) == 1
    assert rows[0]["cwd"] == "/repo"
    assert rows[0]["session_id"] is None


def test_hook_records_event_without_event_name(monkeypatch, tmp_path):
    """A missing hook_event_name must not throw away a valid cwd either."""
    rc, rows = _run_hook(monkeypatch, tmp_path,
                         {"cwd": "/repo", "session_id": "s1"})
    assert rc == 0
    assert len(rows) == 1
    assert rows[0]["cwd"] == "/repo"
    assert rows[0]["event"] is None


def test_hook_skips_event_with_no_cwd(monkeypatch, tmp_path):
    """cwd is the one genuinely required field — no cwd, no signal."""
    rc, rows = _run_hook(monkeypatch, tmp_path,
                         {"session_id": "s1", "hook_event_name": "Stop"})
    assert rc == 0
    assert rows == []
