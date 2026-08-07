"""Tests for src/petridish/scan.

Each test is targeted at a specific behavior described in the scan algorithm.
Sensors are exercised via monkey-patches where we need precise control over
the AgentSignal observations (tests 4-6, 9, 10) or via real fixtures for the
algorithmic buckets / git-side behavior (tests 1, 2, 7, 12).
"""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import tempfile
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any

import pytest

from petridish.config import Config
from petridish.schema import AgentSignal, Radar
from petridish.scan import run_scan, write_scan


# ---------------------------------------------------------------------------
# Hermetic HOME (added by the orchestrator)
#
# run_scan defaults its sensor paths to ~/.claude/projects etc. Before that
# defaulting existed, these tests silently got {} from every sensor; once it
# was fixed they began reading the REAL user home, which both broke their
# assertions and violated the "tests must never read the real ~/.claude"
# invariant in CLAUDE.md. Redirecting HOME keeps them hermetic.
# ---------------------------------------------------------------------------

@pytest.fixture(autouse=True)
def _hermetic_home(tmp_path, monkeypatch):
    fake = tmp_path / "fake_home"
    (fake / ".claude" / "projects").mkdir(parents=True, exist_ok=True)
    (fake / "Library" / "Application Support" / "Code" / "User"
     / "workspaceStorage").mkdir(parents=True, exist_ok=True)
    (fake / ".project-radar").mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("HOME", str(fake))
    return fake


# ---------------------------------------------------------------------------
# Helpers — fixture layout and git plumbing
# ---------------------------------------------------------------------------

def _make_fake_home(tmp_path: Path) -> Path:
    home = tmp_path / "fake_home"
    (home / ".claude" / "projects").mkdir(parents=True, exist_ok=True)
    (home / "Library" / "Application Support" / "Code" / "User" / "workspaceStorage").mkdir(parents=True, exist_ok=True)
    (home / ".project-radar").mkdir(parents=True, exist_ok=True)
    return home


def _git_init(path: Path) -> None:
    subprocess.run(
        ["git", "init", "-q", str(path)], check=True, capture_output=True
    )
    subprocess.run(
        ["git", "config", "user.email", "test@none.invalid"],
        cwd=str(path), check=True, capture_output=True,
    )
    subprocess.run(
        ["git", "config", "user.name", "Test"],
        cwd=str(path), check=True, capture_output=True,
    )


def _git_commit(
    path: Path, message: str = "init", author_date: str | None = None
) -> None:
    (Path(path) / "file.txt").write_text("hi\n")
    subprocess.run(
        ["git", "add", "-A"], cwd=str(path), check=True, capture_output=True
    )
    env = os.environ.copy()
    if author_date is not None:
        env["GIT_AUTHOR_DATE"] = author_date
        env["GIT_COMMITTER_DATE"] = author_date
    subprocess.run(
        ["git", "commit", "-q", "-m", message],
        cwd=str(path), check=True, env=env, capture_output=True,
    )


def _config(roots: list[str], **overrides: Any) -> Config:
    base = {
        "roots": [Path(p) for p in roots],
        "extra_paths": [],
        "author_patterns": ("*",),
        "author_since": "10 years",
        "ignore_dirs": frozenset(),
        "max_depth": 4,
        "bucket_thresholds": {
            "active": 48.0,
            "in_flight": 336.0,
            "stale": 1440.0,
        },
        "category_overrides": {},
    }
    base.update(overrides)
    return Config(**base)  # type: ignore[arg-type]


# ---------------------------------------------------------------------------
# Test 1 — a committed repo produces exactly one project with is_repo True.
# ---------------------------------------------------------------------------

def test_one_commit_yields_one_project(tmp_path: Path):
    home = _make_fake_home(tmp_path)
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    _git_commit(repo, "initial commit")

    cfg = _config([str(repo)])

    radar = run_scan(cfg)

    assert len(radar.projects) == 1
    proj = radar.projects[0]
    assert proj.git.is_repo is True


# ---------------------------------------------------------------------------
# Test 2 — output validates through schema round-trip.
# ---------------------------------------------------------------------------

def test_round_trip_validates(tmp_path: Path):
    home = _make_fake_home(tmp_path)
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    _git_commit(repo)

    cfg = _config([str(repo)])
    radar = run_scan(cfg, now=datetime(2026, 8, 6))

    d = radar.to_dict()
    back = Radar.from_dict(d)

    # Round-trip preserves id, name, path, bucket, is_repo — the fields
    # that drive downstream consumers. Timezone-aware timestamps may shift
    # across serialization, so compare the scalar-ish bits exactly and the
    # commit-at as epoch seconds (which is the natural round-trip unit).
    def _naive_utc(dt):
        if dt is None:
            return None
        return to_utc(dt).replace(tzinfo=None)  # type: ignore[arg-type]

    from petridish.schema import to_utc  # reimport for the helper's scope

    assert _naive_utc(back.updated_at) == _naive_utc(radar.updated_at)
    assert len(back.projects) == 1
    a = back.projects[0]
    b = radar.projects[0]
    assert a.id == b.id
    assert a.name == b.name
    assert a.path == b.path
    assert a.status_bucket == b.status_bucket
    assert a.git.is_repo == b.git.is_repo
    if b.git.mine_last_commit_at is not None:
        assert (a.git.mine_last_commit_at - b.git.mine_last_commit_at).total_seconds() == 0
    if b.git.last_commit_at is not None:
        assert abs(
            (a.git.last_commit_at.timestamp() - b.git.last_commit_at.timestamp())
        ) < 1


# ---------------------------------------------------------------------------
# Test 3 — a sensor that raises does not abort the scan.
# ---------------------------------------------------------------------------

def _boom(*a: Any, **kw: Any) -> dict[str, AgentSignal]:
    raise RuntimeError("simulated sensor failure")


def test_sensor_raise_does_not_abort(tmp_path: Path, monkeypatch):
    home = _make_fake_home(tmp_path)
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    _git_commit(repo)

    cfg = _config([str(repo)])

    monkeypatch.setattr("petridish.scan.claude_scan", _boom)
    monkeypatch.setattr("petridish.scan.copilot_scan", _boom)

    radar = run_scan(cfg, now=datetime(2026, 1, 1))

    # Discovery still produced a project despite both agent sensors failing.
    assert len(radar.projects) == 1
    assert radar.projects[0].git.is_repo


# ---------------------------------------------------------------------------
# Test 4 — signal `at == now` → agent.state == "working" and bucket "active".
# ---------------------------------------------------------------------------

def test_live_agent_is_working(tmp_path: Path, monkeypatch):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    _git_commit(repo)

    cfg = _config([str(repo)])
    now = datetime(2026, 8, 6, 12, 0, 0)
    signal = AgentSignal(
        root=str(repo), at=now, agent="claude",
        session_id="s1", event="chat", raw_cwd=str(repo),
    )

    monkeypatch.setattr("petridish.scan.claude_scan", lambda *a, **k: {str(repo): signal})
    monkeypatch.setattr("petridish.scan.copilot_scan", lambda *a, **k: {})

    radar = run_scan(cfg, now=now)
    proj = radar.projects[0]

    assert proj.agent.state == "working"
    assert proj.status_bucket == "active"
    assert proj.agent.active_agent == "claude"


# ---------------------------------------------------------------------------
# Test 5 — signal ~10 minutes old → agent.state == "recent".
# ---------------------------------------------------------------------------

def test_signal_ten_min_old_is_recent(tmp_path: Path, monkeypatch):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    _git_commit(repo)

    cfg = _config([str(repo)])
    now = datetime(2026, 8, 6, 12, 10, 30)
    ten_min_ago = datetime(2026, 8, 6, 12, 0, 0)
    signal = AgentSignal(
        root=str(repo), at=ten_min_ago, agent="copilot",
        session_id="s2", event="chat", raw_cwd=str(repo),
    )

    monkeypatch.setattr("petridish.scan.claude_scan", lambda *a, **k: {})
    monkeypatch.setattr("petridish.scan.copilot_scan", lambda *a, **k: {str(repo): signal})

    radar = run_scan(cfg, now=now)
    proj = radar.projects[0]

    assert proj.agent.state == "recent"
    assert proj.status_bucket == "active"


# ---------------------------------------------------------------------------
# Test 6 — signal ~2 hours old → agent.state == "idle".
# ---------------------------------------------------------------------------

def test_signal_two_hours_old_is_idle(tmp_path: Path, monkeypatch):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)

    cfg = _config([str(repo)])
    now = datetime(2026, 8, 6, 14, 0, 0)
    two_hours_ago = datetime(2026, 8, 6, 12, 0, 0)
    signal = AgentSignal(
        root=str(repo), at=two_hours_ago, agent="claude",
        session_id="s3", event="chat", raw_cwd=str(repo),
    )

    monkeypatch.setattr("petridish.scan.claude_scan", lambda *a, **k: {str(repo): signal})
    monkeypatch.setattr("petridish.scan.copilot_scan", lambda *a, **k: {})

    radar = run_scan(cfg, now=now)
    proj = radar.projects[0]

    assert proj.agent.state == "idle"


# ---------------------------------------------------------------------------
# Test 7 — bucket thresholds: 5 days → in_flight; 30 days → stale; 100 days
#          → cold. Only git commits are in play (no agent signals).
# ---------------------------------------------------------------------------

@pytest.mark.parametrize(
    "days_ago,expected_bucket",
    [
        (5, "in_flight"),   # 120h < 336h
        (30, "stale"),      # 720h < 1440h
        (100, "cold"),      # 2400h ≥ 1440h
    ],
)
def test_bucketing_git_only(tmp_path: Path, days_ago: int, expected_bucket: str):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)

    commit_time = datetime(2026, 8, 6) - timedelta(days=days_ago)
    # Pin date format `YYYY-MM-DDTHH:MM:SSZ` (UTC).
    _git_commit(repo, author_date=commit_time.strftime("%Y-%m-%dT%H:%M:%SZ"))

    cfg = _config([str(repo)])
    # Scan "now" is at the epoch day so the commit age is exactly `days_ago`.
    now = datetime(2026, 8, 6)
    radar = run_scan(cfg, now=now)

    assert radar.projects[0].status_bucket == expected_bucket


# ---------------------------------------------------------------------------
# Test 8 — "working" agent forces "active" even when git date is 100 days old.
# ---------------------------------------------------------------------------

def test_working_overrides_cold_git(tmp_path: Path, monkeypatch):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)

    hundred_days_ago = datetime(2026, 4, 28)
    _git_commit(
        repo, author_date=hundred_days_ago.strftime("%Y-%m-%dT%H:%M:%SZ")
    )

    cfg = _config([str(repo)])
    now = datetime(2026, 8, 6)
    live_signal = AgentSignal(
        root=str(repo), at=now, agent="claude",
        session_id="s1", event="chat", raw_cwd=str(repo),
    )

    monkeypatch.setattr("petridish.scan.claude_scan", lambda *a, **k: {str(repo): live_signal})
    monkeypatch.setattr("petridish.scan.copilot_scan", lambda *a, **k: {})

    radar = run_scan(cfg, now=now)
    assert radar.projects[0].status_bucket == "active"


# ---------------------------------------------------------------------------
# Test 9 — signal whose root is outside config.roots still appears as a
# project.
# ---------------------------------------------------------------------------

def test_signal_outside_roots_still_appears(tmp_path: Path, monkeypatch):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    _git_commit(repo)

    outside = tmp_path / "outside"
    outside.mkdir()

    cfg = _config([str(repo)])
    now = datetime(2026, 8, 6)
    live_signal = AgentSignal(
        root=str(outside), at=now, agent="copilot",
        session_id="s9", event="chat", raw_cwd=str(outside),
    )

    monkeypatch.setattr("petridish.scan.copilot_scan", lambda *a, **k: {str(outside): live_signal})

    radar = run_scan(cfg, now=now)
    by_name = {p.name: p for p in radar.projects}
    assert "repo" in by_name
    assert "outside" in by_name


# ---------------------------------------------------------------------------
# Test 10 — two sensors for the same root: newest at wins.
# ---------------------------------------------------------------------------

def test_merge_newest_at_wins(tmp_path: Path, monkeypatch):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    _git_commit(repo)

    cfg = _config([str(repo)])
    now = datetime(2026, 8, 6, 12)
    older = now - timedelta(days=10)
    fresher = now - timedelta(hours=2)

    old_signal = AgentSignal(
        root=str(repo), at=older, agent="claude",
        session_id="s1", event="e1", raw_cwd=str(repo),
    )
    new_signal = AgentSignal(
        root=str(repo), at=fresher, agent="copilot",
        session_id="s2", event="e2", raw_cwd=str(repo),
    )

    monkeypatch.setattr("petridish.scan.claude_scan", lambda *a, **k: {str(repo): old_signal})
    monkeypatch.setattr("petridish.scan.copilot_scan", lambda *a, **k: {str(repo): new_signal})

    radar = run_scan(cfg, now=now)
    assert len(radar.projects) == 1
    assert radar.projects[0].agent.active_agent == "copilot"


# ---------------------------------------------------------------------------
# Test 11 — id is stable for the same path, different between paths.
# ---------------------------------------------------------------------------

def test_id_stable_across_runs(tmp_path: Path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    _git_commit(repo)

    cfg = _config([str(repo)])

    radar1 = run_scan(cfg, now=datetime(2026, 8, 6))
    radar2 = run_scan(cfg, now=datetime(2026, 8, 7))

    assert radar1.projects[0].id == radar2.projects[0].id


def test_id_differs_between_paths(tmp_path: Path):
    a = tmp_path / "proj-a"
    b = tmp_path / "proj-b"
    a.mkdir(); b.mkdir()
    for p in (a, b):
        _git_init(p)
        _git_commit(p)

    cfg = _config([str(a), str(b)])
    radar = run_scan(cfg, now=datetime(2026, 8, 6))

    ids = {p.id for p in radar.projects}
    assert len(ids) == 2


# ---------------------------------------------------------------------------
# Test 12 — write_scan produces a file loadable by petridish.schema.read_json.
# ---------------------------------------------------------------------------

def test_write_scan_round_trip(tmp_path: Path, monkeypatch):
    from petridish import schema as radar_schema

    repo = tmp_path / "repo"
    repo.mkdir()
    _git_init(repo)
    _git_commit(repo)

    cfg = _config([str(repo)])
    out_path = tmp_path / "petridish.json"

    radar = write_scan(cfg, str(out_path), now=datetime(2026, 8, 6))

    # read_json takes a PATH, not a parsed dict — as delegated this passed the
    # decoded blob and raised TypeError.
    blob = json.loads(out_path.read_text("utf-8"))
    assert blob["schema_version"] == 1

    back = radar_schema.read_json(out_path)
    assert back == radar


# ---------------------------------------------------------------------------
# Default sensor paths (added by the orchestrator)
#
# As delegated, claude_dir/copilot_dir/events_path were declared but never
# defaulted. In production each sensor received None, raised, and was swallowed
# by run_scan's except blocks into {} — yielding a projects.json with zero
# agent activity that looked entirely healthy. Every existing test missed it
# because they all monkeypatch the sensors or pass explicit fixture dirs.
# ---------------------------------------------------------------------------


def test_sensors_receive_real_paths_when_not_injected(tmp_path, monkeypatch):
    """run_scan must default the sensor paths, never hand them None."""
    seen = {}

    def _spy(name):
        def _f(target, config, **kwargs):
            seen[name] = target
            return {}
        return _f

    monkeypatch.setattr("petridish.scan.claude_scan", _spy("claude"))
    monkeypatch.setattr("petridish.scan.copilot_scan", _spy("copilot"))
    monkeypatch.setattr("petridish.scan._read_events", _spy("events"))

    run_scan(_config([str(tmp_path)]))

    assert seen["claude"] is not None, "claude_dir was left as None"
    assert seen["copilot"] is not None, "copilot_dir was left as None"
    assert seen["events"] is not None, "events_path was left as None"

    assert "projects" in str(seen["claude"])
    assert "workspaceStorage" in str(seen["copilot"])
    assert "events.ndjson" in str(seen["events"])


# ---------------------------------------------------------------------------
# Test 13 — category_overrides actually applies.
#
# Regression: _build_project looked the override up with a Path key while
# Config.category_overrides is a dict[str, str] keyed by path strings. Path
# and str hash differently, so the lookup missed every single time and the
# whole feature was inert — with no test covering it either way.
# ---------------------------------------------------------------------------

def test_category_override_applies_to_matching_root(tmp_path: Path):
    home = _make_fake_home(tmp_path)
    repo = tmp_path / "projects" / "myrepo"
    repo.mkdir(parents=True)
    _git_init(repo)
    _git_commit(repo, "initial")

    resolved = str(repo.resolve())
    cfg = _config(
        [str(tmp_path / "projects")],
        category_overrides={resolved: "special"},
    )

    radar = run_scan(cfg, claude_dir=str(home / ".claude" / "projects"))
    proj = next(p for p in radar.projects if p.path == resolved)

    # Without the fix this is the parent directory name ("projects").
    assert proj.category == "special"


def test_category_falls_back_to_parent_dir_name(tmp_path: Path):
    home = _make_fake_home(tmp_path)
    repo = tmp_path / "projects" / "plainrepo"
    repo.mkdir(parents=True)
    _git_init(repo)
    _git_commit(repo, "initial")

    cfg = _config([str(tmp_path / "projects")])
    radar = run_scan(cfg, claude_dir=str(home / ".claude" / "projects"))
    proj = next(p for p in radar.projects if p.path == str(repo.resolve()))

    assert proj.category == "projects"
