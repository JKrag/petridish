"""Tests for ``src/petridish/cli.py``.

All fixtures live in a temporary directory (via ``tmp_path`` +
``monkeypatch.setenv("HOME", ...)``) so the real ``~/.claude`` and
``~/.petridish`` are never touched by the tests.  Each test builds its
fixture state file via :func:`petridish.schema.write_atomic` and hands the
temporary path through ``--state``.
"""

from __future__ import annotations

import json
import sys
from datetime import datetime, timezone
from pathlib import Path

import pytest

from petridish.cli import main
from petridish.schema import (
    AgentState,
    GitState,
    Project,
    Radar,
    write_atomic,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _dt(y: int, mo: int, d: int, h: int = 0, mi: int = 0, s: int = 0) -> datetime:
    """Convenience for tz-aware UTC datetimes used as fixtures."""
    return datetime(y, mo, d, h, mi, s, tzinfo=timezone.utc)


def _write_fixture(tmp_path: Path, projects: list[Project]) -> Path:
    """Write a realistic ``projects.json`` into tmp_path and return its path."""
    state_path = tmp_path / "projects.json"
    radar = Radar(
        updated_at=_dt(2026, 8, 6, 12, 0, 0),
        scan_duration_ms=123,
        projects=tuple(projects),
    )
    write_atomic(radar, state_path)
    return state_path


def _make_project(
    name: str,
    path: str,
    *,
    bucket: str = "active",
    is_foreign: bool = False,
    last_activity_at: datetime | None = _dt(2026, 8, 6, 11, 0, 0),
    agent_label: str | None = "claude",
) -> Project:
    return Project(
        id=f"hash-of-{name}",
        name=name,
        path=path,
        category="personal",
        is_foreign=is_foreign,
        git=GitState(is_repo=True, branch="main"),
        agent=AgentState(state="idle", active_agent=agent_label),
        last_activity_at=last_activity_at,
        status_bucket=bucket,
    )


@pytest.fixture(autouse=True)
def _isolate_home(tmp_path: Path, monkeypatch):
    """Redirect HOME so the real ~/.claude is never touched by tests."""
    monkeypatch.setenv("HOME", str(tmp_path))


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

def test_list_shows_projects(tmp_path, capsys):
    """list on a fixture prints a table containing a known project name;
    returns 0."""
    state = _write_fixture(
        tmp_path, [_make_project("project-radar", "/tmp/project-radar")],
    )
    rc = main(["--state", str(state), "list"])
    assert rc == 0
    captured = capsys.readouterr()
    assert "project-radar" in captured.out


def test_list_missing_state_returns_one(capsys):
    """list on a missing state file returns 1 and does not raise."""
    rc = main(["--state", "/nope/projects.json", "list"])
    assert rc == 1
    err = capsys.readouterr().err
    assert "no state file at /nope/projects.json" in err


def test_list_json_is_valid(tmp_path, capsys):
    """list --json emits valid JSON parseable by json.loads."""
    state = _write_fixture(
        tmp_path, [_make_project("myproject", "/tmp/myproject")],
    )
    rc = main(["--state", str(state), "list", "--json"])
    assert rc == 0
    out = capsys.readouterr().out
    payload = json.loads(out)
    assert isinstance(payload, list)
    names = {p["name"] for p in payload}
    assert "myproject" in names


def test_list_bucket_filter(tmp_path, capsys):
    """list --bucket active shows only active projects."""
    state = _write_fixture(
        tmp_path,
        [
            _make_project("alpha", "/x/alpha", bucket="active"),
            _make_project("blue", "/x/blue", bucket="cold"),
        ],
    )
    rc = main(["--state", str(state), "list", "--bucket", "active"])
    assert rc == 0
    out = capsys.readouterr().out
    assert "alpha" in out
    assert "blue" not in out


def test_list_hides_foreign_by_default(tmp_path, capsys):
    """list hides an is_foreign project; list --all shows it."""
    state = _write_fixture(
        tmp_path,
        [
            _make_project("mine", "/x/mine"),
            _make_project("theirs", "/y/theirs", is_foreign=True),
        ],
    )

    rc = main(["--state", str(state), "list"])
    assert rc == 0
    out = capsys.readouterr().out
    assert "mine" in out
    assert "theirs" not in out

    rc = main(["--state", str(state), "list", "--all"])
    assert rc == 0
    out = capsys.readouterr().out
    assert "theirs" in out


def test_path_exact_name(tmp_path, capsys):
    """path <exact name> prints exactly that project's path; returns 0."""
    state = _write_fixture(
        tmp_path,
        [_make_project("project-radar", "/home/me/repos/project-radar")],
    )
    rc = main(["--state", str(state), "path", "project-radar"])
    assert rc == 0
    out = capsys.readouterr().out
    assert out.strip() == "/home/me/repos/project-radar"


def test_path_case_insensitive_substr(tmp_path, capsys):
    """path <substring> matches case-insensitively."""
    state = _write_fixture(
        tmp_path,
        [_make_project("ProjectRadar", "/home/me/repos/project-radar")],
    )
    rc = main(["--state", str(state), "path", "radar"])
    assert rc == 0
    out = capsys.readouterr().out
    assert out.strip() == "/home/me/repos/project-radar"


def test_path_no_match(tmp_path, capsys):
    """path <no match> returns 1 and prints nothing to stdout."""
    state = _write_fixture(
        tmp_path,
        [_make_project("project-radar", "/home/me/repos/project-radar")],
    )
    rc = main(["--state", str(state), "path", "zzz_no_match_zzz_xyz"])
    assert rc == 1
    out = capsys.readouterr().out
    assert out == ""


def test_doctor_missing_state_returns_nonzero(capsys):
    """doctor returns non-zero when the state file is missing."""
    rc = main(["--state", "/nope/projects.json", "doctor"])
    assert rc != 0
    out = capsys.readouterr().out
    # The "state" check should fire (reporting failure).
    assert "state" in out


def test_doctor_hook_present_and_absent(tmp_path, capsys):
    """doctor reports the hook as present when a fake settings.json contains
    a command with the ``# petridish`` marker, and absent when that file
    contains a ``swab-hook`` command WITHOUT the marker."""
    settings_dir = tmp_path / ".claude"
    settings_dir.mkdir(parents=True, exist_ok=True)
    settings_path = settings_dir / "settings.json"

    # Case A: marker present.
    settings_path.write_text(
        json.dumps({
            "hooks": {
                "pre:commit": {
                    "command": "swab-hook --state /tmp/s.json # petridish",
                },
            }
        })
    )
    out = _capture_doctor(["--state", "/nope/projects.json", "doctor"])
    lines = [ln for ln in out.splitlines() if ln.startswith("ok:") or
             ln.startswith("fail:")]
    hook_lines = [l for l in lines if "hook" in l]
    assert any("ok: hook" in l for l in hook_lines), (
        f"expected 'ok: hook' in {hook_lines!r}"
    )

    # Case B: ``swab-hook`` string but no marker.
    settings_path.write_text(
        json.dumps({
            "hooks": {
                "pre:commit": {
                    "command": "swab-hook --state /tmp/s.json",
                },
            }
        })
    )
    out = _capture_doctor(["--state", "/nope/projects.json", "doctor"])
    hook_lines = [l for l in out.splitlines() if "hook" in l]
    assert any("fail: hook" in l for l in hook_lines), (
        f"expected 'fail: hook' in {hook_lines!r}"
    )


def _capture_doctor(argv):
    """Run ``doctor`` and return its stdout without a capsys fixture."""
    import io
    from unittest.mock import patch

    old_stdout = sys.stdout
    buf = io.StringIO()
    with patch.object(sys, "stdout", buf):
        main(argv)
    return buf.getvalue()


def test_main_no_subcommand_returns_nonzero(capsys):
    """main([]) with no subcommand returns a non-zero code and prints help."""
    with pytest.raises(SystemExit) as exc_info:
        main([])
    assert exc_info.value.code != 0
    captured = capsys.readouterr()
    # argparse prints help/errors to stderr when a required subcommand is
    # missing. Check both streams so the assertion is stable across versions.
    combined = (captured.out + captured.err).lower()
    assert "usage" in combined or "required" in combined


def test_list_shows_agent_name_even_when_idle(tmp_path, capsys, monkeypatch):
    """An idle project must still name its agent.

    As delegated, the table printed a bare "idle" and dropped active_agent —
    losing the one fact you act on (which agent, and thus which session to
    resume) for exactly the projects that are not currently live.
    """
    from datetime import datetime, timezone
    from petridish.schema import AgentState, Project, Radar, write_atomic

    monkeypatch.setenv("HOME", str(tmp_path))
    state = tmp_path / "p.json"
    radar = Radar(
        updated_at=datetime(2026, 8, 6, tzinfo=timezone.utc),
        projects=(
            Project(
                id="abc123abc123", name="dormant", path="/tmp/dormant",
                category="misc",
                agent=AgentState(state="idle", active_agent="claude-code",
                                 session_id="sess-1"),
                status_bucket="stale",
            ),
        ),
    )
    write_atomic(radar, state)

    rc = main(["--state", str(state), "list"])
    out = capsys.readouterr().out

    assert rc == 0
    assert "claude-code" in out, "idle project dropped its agent name"
    assert "idle" in out
