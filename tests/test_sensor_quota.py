"""Tests for ``src/petridish/sensors/quota.py``.

This sensor reads an **undocumented internal file of another program**
(``~/.claude/last-status.json``). Anthropic never promised its shape, so the
interesting tests here are not the happy path — they are every way a Claude
Code upgrade could change it. Each must yield ``None`` or a partial
:class:`QuotaState`, never an exception, because a raise here would take out
the whole scan tick (invariant 5).

Nothing reads the real ``~/.claude``: ``read_quota`` takes an explicit ``home``.
"""

from __future__ import annotations

import json
import os
import sys
from datetime import datetime, timedelta, timezone

import pytest

from petridish.schema import QuotaState
from petridish.sensors.quota import STATUS_FILE, parse_status, read_quota

NOW = datetime(2026, 8, 9, 6, 32, 11, tzinfo=timezone.utc)


def _epoch(dt: datetime) -> int:
    return int(dt.timestamp())


def _real_shape() -> dict:
    """The exact shape observed in a live ~/.claude/last-status.json."""
    return {
        "ts": "2026-08-09T06:32:11Z",
        "rate_limits": {
            "five_hour": {
                "used_percentage": 9,
                "resets_at": _epoch(NOW + timedelta(hours=5)),
            },
            "seven_day": {
                "used_percentage": 86,
                "resets_at": _epoch(NOW + timedelta(days=2)),
            },
        },
        "context_window": {
            "total_input_tokens": 276035,
            "context_window_size": 1000000,
            "used_percentage": 28,
            "remaining_percentage": 72,
        },
    }


def _write(home, payload) -> None:
    path = home / ".claude"
    path.mkdir(parents=True, exist_ok=True)
    (path / "last-status.json").write_text(
        payload if isinstance(payload, str) else json.dumps(payload)
    )


# ---------------------------------------------------------------------------
# Happy path
# ---------------------------------------------------------------------------

def test_parses_the_observed_live_shape(tmp_path):
    _write(tmp_path, _real_shape())
    q = read_quota(str(tmp_path), now=NOW)
    assert q is not None
    assert q.five_hour_used_pct == 9
    assert q.seven_day_used_pct == 86
    assert q.context_used_pct == 28
    assert q.measured_at == NOW
    assert q.five_hour_resets_at == NOW + timedelta(hours=5)
    assert q.seven_day_resets_at == NOW + timedelta(days=2)


def test_status_file_location_is_under_dot_claude():
    assert STATUS_FILE == os.path.join(".claude", "last-status.json")


# ---------------------------------------------------------------------------
# Absence and corruption. All ordinary; none may raise.
# ---------------------------------------------------------------------------

def test_missing_file_is_none(tmp_path):
    assert read_quota(str(tmp_path), now=NOW) is None


def test_missing_home_is_none():
    assert read_quota("/nonexistent-home-xyz", now=NOW) is None


def test_unreadable_file_is_none(tmp_path):
    _write(tmp_path, _real_shape())
    path = tmp_path / ".claude" / "last-status.json"
    path.chmod(0o000)
    try:
        assert read_quota(str(tmp_path), now=NOW) is None
    finally:
        path.chmod(0o644)  # so tmp_path cleanup works


@pytest.mark.parametrize(
    "raw",
    [
        "",
        "{",
        "not json at all",
        '{"ts": "2026-08-09T06:32:11Z", "rate_lim',  # truncated mid-write
    ],
)
def test_malformed_json_is_none(tmp_path, raw):
    """The file is rewritten as sessions run, so a torn read is expected."""
    _write(tmp_path, raw)
    assert read_quota(str(tmp_path), now=NOW) is None


@pytest.mark.parametrize("payload", [None, [], "string", 42, True])
def test_non_mapping_payload_is_none(payload):
    assert parse_status(payload, now=NOW) is None


def test_empty_object_is_none():
    """No recognisable field means absence, not a row of blanks."""
    assert parse_status({}, now=NOW) is None


def test_payload_with_only_unknown_keys_is_none():
    assert parse_status({"something_new": {"a": 1}}, now=NOW) is None


# ---------------------------------------------------------------------------
# Partial truth. A schema change that drops one field must not lose the others.
# ---------------------------------------------------------------------------

def test_a_dropped_window_leaves_the_other_intact():
    payload = _real_shape()
    del payload["rate_limits"]["five_hour"]
    q = parse_status(payload, now=NOW)
    assert q is not None
    assert q.five_hour_used_pct is None
    assert q.five_hour_resets_at is None
    assert q.seven_day_used_pct == 86


def test_renamed_rate_limits_key_still_yields_the_context_window():
    payload = _real_shape()
    payload["limits"] = payload.pop("rate_limits")
    q = parse_status(payload, now=NOW)
    assert q is not None
    assert q.five_hour_used_pct is None
    assert q.context_used_pct == 28


def test_wrong_nesting_types_are_ignored_not_fatal():
    """Every level that we index into could stop being a dict."""
    for payload in (
        {"rate_limits": "surprise", "context_window": {"used_percentage": 28}},
        {"rate_limits": {"five_hour": ["not", "a", "dict"]},
         "context_window": {"used_percentage": 28}},
        {"rate_limits": {"five_hour": {"used_percentage": 9}},
         "context_window": 12},
    ):
        q = parse_status(payload, now=NOW)
        assert q is not None  # something survived in each


# ---------------------------------------------------------------------------
# Value sanity. A number of the wrong kind must be dropped, not rendered.
# ---------------------------------------------------------------------------

@pytest.mark.parametrize("bad", [-1, 101, 1000, "9", None, [9]])
def test_out_of_range_or_non_numeric_percentages_are_dropped(bad):
    payload = {"rate_limits": {"five_hour": {"used_percentage": bad}},
               "context_window": {"used_percentage": 28}}
    q = parse_status(payload, now=NOW)
    assert q is not None
    assert q.five_hour_used_pct is None


def test_boolean_percentage_is_rejected():
    """``bool`` is an ``int`` subclass; ``True`` must not become 1%."""
    payload = {"rate_limits": {"five_hour": {"used_percentage": True}},
               "context_window": {"used_percentage": 28}}
    assert parse_status(payload, now=NOW).five_hour_used_pct is None


def test_zero_percent_survives():
    """0 is a real reading and must not be swallowed by a falsiness check."""
    payload = {"rate_limits": {"five_hour": {"used_percentage": 0,
                                            "resets_at": _epoch(NOW)}}}
    q = parse_status(payload, now=NOW)
    assert q is not None
    assert q.five_hour_used_pct == 0


@pytest.mark.parametrize(
    "bad_epoch",
    [
        1786275000000,   # milliseconds, not seconds — the classic units bug
        0,               # 1970
        -1,
        "1786275000",    # string
        None,
    ],
)
def test_implausible_reset_timestamps_are_dropped(bad_epoch):
    """A reset 20000 days out is worse to render than nothing at all."""
    payload = {"rate_limits": {"five_hour": {"used_percentage": 9,
                                            "resets_at": bad_epoch}}}
    q = parse_status(payload, now=NOW)
    assert q is not None
    assert q.five_hour_used_pct == 9  # the good field still survives
    assert q.five_hour_resets_at is None


def test_boolean_reset_timestamp_is_rejected():
    payload = {"rate_limits": {"five_hour": {"used_percentage": 9,
                                            "resets_at": True}}}
    assert parse_status(payload, now=NOW).five_hour_resets_at is None


@pytest.mark.parametrize("bad_ts", ["", "yesterday", "2026-13-45T99:99:99Z", 12345, None])
def test_unparseable_measured_at_is_dropped(bad_ts):
    payload = _real_shape()
    payload["ts"] = bad_ts
    q = parse_status(payload, now=NOW)
    assert q is not None
    assert q.measured_at is None
    assert q.seven_day_used_pct == 86  # everything else survives


def test_a_naive_timestamp_is_assumed_utc():
    payload = _real_shape()
    payload["ts"] = "2026-08-09T06:32:11"
    assert parse_status(payload, now=NOW).measured_at == NOW


def test_absurdly_old_measured_at_is_dropped():
    """A year-old ts means we misread the field, not that the file is a year old."""
    payload = _real_shape()
    payload["ts"] = "2020-01-01T00:00:00Z"
    assert parse_status(payload, now=NOW).measured_at is None


# ---------------------------------------------------------------------------
# The result is the schema type, and round-trips through JSON.
# ---------------------------------------------------------------------------

def test_result_round_trips_through_the_wire_format(tmp_path):
    _write(tmp_path, _real_shape())
    q = read_quota(str(tmp_path), now=NOW)
    assert QuotaState.from_dict(json.loads(json.dumps(q.to_dict()))) == q


def test_default_quota_state_round_trips():
    empty = QuotaState()
    assert QuotaState.from_dict(json.loads(json.dumps(empty.to_dict()))) == empty


if __name__ == "__main__":  # pragma: no cover
    sys.exit(__import__("pytest").main([__file__, "-v"]))
