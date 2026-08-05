"""Tests for ``src/radar/schema.py`` — the frozen contract (M1).

Everything downstream codes against this module, so these tests are
deliberately strict about the round-trip and the atomic-write guarantee.
"""

from __future__ import annotations

import dataclasses
import json
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from radar.schema import (
    SCHEMA_VERSION,
    AgentSignal,
    AgentState,
    GitState,
    Project,
    Radar,
    read_json,
    to_utc,
    utcnow,
    write_atomic,
)

FIXTURE = Path(__file__).parent / "fixtures" / "projects.golden.json"


def _dt(y, mo, d, h, mi, s) -> datetime:
    return datetime(y, mo, d, h, mi, s, tzinfo=timezone.utc)


def _golden_radar() -> Radar:
    """The in-memory twin of ``projects.golden.json``."""
    return Radar(
        updated_at=_dt(2026, 8, 5, 22, 45, 0),
        scan_duration_ms=412,
        projects=(
            Project(
                id="a1b2c3d4e5f6",
                name="project-radar",
                path="/Users/jankrag/repos/JKrag/project-radar",
                category="JKrag",
                is_foreign=False,
                git=GitState(
                    is_repo=True,
                    branch="master",
                    is_dirty=True,
                    uncommitted_files=4,
                    last_commit_at=_dt(2026, 8, 4, 14, 10, 0),
                    mine_last_commit_at=_dt(2026, 8, 4, 14, 10, 0),
                    github_url="https://github.com/JKrag/project-radar",
                ),
                agent=AgentState(
                    state="working",
                    active_agent="claude-code",
                    last_event="PreToolUse",
                    last_event_at=_dt(2026, 8, 5, 22, 44, 12),
                    session_id="6f4a8f6e-ae3c-4bf6-846b-d1fecb5742a6",
                ),
                last_activity_at=_dt(2026, 8, 5, 22, 44, 12),
                status_bucket="active",
            ),
        ),
    )


# ---------------------------------------------------------------------------
# Round-tripping
# ---------------------------------------------------------------------------

def test_roundtrip_fully_populated():
    r = _golden_radar()
    assert Radar.from_dict(r.to_dict()) == r


def test_roundtrip_minimal_all_optionals_none():
    r = Radar(
        updated_at=_dt(2026, 1, 1, 0, 0, 0),
        projects=(Project(id="x", name="x", path="/x", category="misc"),),
    )
    back = Radar.from_dict(r.to_dict())

    assert back == r
    assert back.projects[0].git.is_repo is False
    assert back.projects[0].agent.state == "idle"
    assert back.projects[0].last_activity_at is None


def test_roundtrip_empty_project_list():
    r = Radar(updated_at=_dt(2026, 1, 1, 0, 0, 0))
    assert Radar.from_dict(r.to_dict()) == r


def test_projects_is_tuple_after_from_dict():
    """A list would break frozen-ness and round-trip equality alike."""
    back = Radar.from_dict(_golden_radar().to_dict())
    assert isinstance(back.projects, tuple)


# ---------------------------------------------------------------------------
# The golden fixture is the cross-module contract
# ---------------------------------------------------------------------------

def test_to_dict_matches_golden_fixture_exactly():
    produced = _golden_radar().to_dict()
    expected = json.loads(FIXTURE.read_text(encoding="utf-8"))
    assert produced == expected


def test_golden_fixture_key_order_matches_serialiser():
    """Frontends read this file by eye; stable key order keeps diffs readable."""
    produced = _golden_radar().to_dict()
    expected = json.loads(FIXTURE.read_text(encoding="utf-8"))

    assert list(produced) == list(expected)
    assert list(produced["projects"][0]) == list(expected["projects"][0])
    assert list(produced["projects"][0]["git"]) == list(expected["projects"][0]["git"])
    assert list(produced["projects"][0]["agent"]) == list(expected["projects"][0]["agent"])


def test_fixture_parses_back_to_equal_radar():
    loaded = Radar.from_json(FIXTURE.read_text(encoding="utf-8"))
    assert loaded == _golden_radar()


def test_schema_version_is_one():
    assert SCHEMA_VERSION == 1
    assert _golden_radar().to_dict()["schema_version"] == 1


# ---------------------------------------------------------------------------
# Datetime handling
# ---------------------------------------------------------------------------

def test_datetimes_serialise_with_trailing_z():
    d = _golden_radar().to_dict()
    assert d["updated_at"].endswith("Z")
    assert d["projects"][0]["git"]["last_commit_at"].endswith("Z")


def test_parsed_datetimes_are_timezone_aware_utc():
    back = Radar.from_dict(_golden_radar().to_dict())
    assert back.updated_at.tzinfo is not None
    assert back.updated_at.utcoffset() == timedelta(0)


def test_naive_datetime_is_treated_as_utc_not_local():
    """Guessing local time here would silently shift every timestamp."""
    naive = datetime(2026, 8, 5, 22, 45, 0)
    r = Radar(updated_at=naive)
    assert r.to_dict()["updated_at"] == "2026-08-05T22:45:00Z"


def test_sub_second_precision_is_truncated():
    """Documented, deliberate: timestamps are second-resolution."""
    r = Radar(updated_at=datetime(2026, 8, 5, 22, 45, 0, 999999, tzinfo=timezone.utc))
    assert r.to_dict()["updated_at"] == "2026-08-05T22:45:00Z"


def test_to_utc_converts_from_other_offset():
    other = datetime(2026, 8, 5, 23, 45, 0, tzinfo=timezone(timedelta(hours=1)))
    assert to_utc(other) == _dt(2026, 8, 5, 22, 45, 0)


def test_utcnow_is_aware_and_second_resolution():
    now = utcnow()
    assert now.tzinfo is not None
    assert now.microsecond == 0


def test_from_dict_rejects_missing_updated_at():
    with pytest.raises(ValueError, match="updated_at"):
        Radar.from_dict({"projects": []})


# ---------------------------------------------------------------------------
# Atomic write
# ---------------------------------------------------------------------------

def test_write_atomic_creates_readable_file(tmp_path):
    target = tmp_path / "projects.json"
    r = _golden_radar()

    write_atomic(r, target)

    assert target.is_file()
    assert read_json(target) == r


def test_write_atomic_leaves_no_tmp_file(tmp_path):
    target = tmp_path / "projects.json"
    write_atomic(_golden_radar(), target)

    leftovers = [p.name for p in tmp_path.iterdir()]
    assert leftovers == ["projects.json"]
    assert not any(n.endswith(".tmp") for n in leftovers)


def test_write_atomic_overwrites_existing(tmp_path):
    target = tmp_path / "projects.json"
    target.write_text("stale garbage", encoding="utf-8")

    r = _golden_radar()
    write_atomic(r, target)

    assert read_json(target) == r
    assert [p.name for p in tmp_path.iterdir()] == ["projects.json"]


def test_write_atomic_creates_parent_directory(tmp_path):
    target = tmp_path / "nested" / "deeper" / "projects.json"
    write_atomic(_golden_radar(), target)
    assert target.is_file()


def test_write_atomic_cleans_up_tmp_on_failure(tmp_path, monkeypatch):
    """A failed write must not strand a .tmp file for the next run to trip on."""
    target = tmp_path / "projects.json"

    def boom(*a, **kw):
        raise RuntimeError("disk on fire")

    monkeypatch.setattr("radar.schema.os.replace", boom)

    with pytest.raises(RuntimeError):
        write_atomic(_golden_radar(), target)

    assert list(tmp_path.iterdir()) == []


# ---------------------------------------------------------------------------
# Frozen contract
# ---------------------------------------------------------------------------

@pytest.mark.parametrize(
    "obj,fieldname,value",
    [
        (GitState(), "branch", "main"),
        (AgentState(), "state", "working"),
        (AgentSignal(root="/x", at=utcnow(), agent="claude-code"), "agent", "copilot"),
        (Project(id="i", name="n", path="/p", category="c"), "name", "other"),
        (Radar(updated_at=utcnow()), "scan_duration_ms", 5),
    ],
)
def test_records_are_frozen(obj, fieldname, value):
    with pytest.raises(dataclasses.FrozenInstanceError):
        setattr(obj, fieldname, value)


def test_agent_signal_carries_resolved_root_and_raw_cwd():
    """The monorepo-attribution contract: root is resolved, raw_cwd is evidence."""
    sig = AgentSignal(
        root="/repo",
        at=utcnow(),
        agent="claude-code",
        session_id="s1",
        event="PreToolUse",
        raw_cwd="/repo/packages/core",
    )
    assert sig.root == "/repo"
    assert sig.raw_cwd == "/repo/packages/core"


def test_agent_signal_is_not_serialised_into_radar():
    """AgentSignal is internal — it must not leak into projects.json."""
    assert "raw_cwd" not in json.dumps(_golden_radar().to_dict())
