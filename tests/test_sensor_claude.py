"""Tests for ``petridish.sensors.claude`` — the transcript scanner.

Every test writes real fixture ``.jsonl`` files under ``tmp_path`` and, when it
needs a specific mtime, calls :func:`os.utime`. We never touch the real
``~/.claude/`` directory.

The test names map 1:1 to the contract in ``src/petridish/sensors/claude.py``;
see the module docstring there for the full invariants list.
"""

from __future__ import annotations

import json
import os
import time
from datetime import timezone
from pathlib import Path

import pytest

from petridish.config import Config
from petridish.sensors.claude import scan


# ---------------------------------------------------------------------------
# Fixture helpers
# ---------------------------------------------------------------------------

def _write_lines(path: Path, lines: list[dict]) -> None:
    """Serialise a list of dicts as one JSON object per line, no trailing newline."""
    path.parent.mkdir(parents=True, exist_ok=True)
    lines_text = "\n".join(json.dumps(d) for d in lines)
    path.write_text(lines_text, encoding="utf-8")


def _fresh_mtime(path: Path) -> None:
    """Pin ``path`` to the current wall clock. Useful when mtimes get pinned
    by an earlier :func:`os.utime` call and we want a test's file to look
    "just touched"."""
    os.utime(path)


def _make_config(roots) -> Config:
    return Config(roots=tuple(roots))


# ---------------------------------------------------------------------------
# 1. Single transcript → one signal, correct root + session_id
# ---------------------------------------------------------------------------

def test_one_transcript_yields_one_signal(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()  # no .git — resolve_root will return cwd itself

    claude_dir = tmp_path / "claude_projects"
    project_dir = claude_dir / "-Users-me-my-project-"  # noise dir name
    project_dir.mkdir(parents=True)
    transcript = project_dir / "abc-def-session.jsonl"

    _write_lines(transcript, [
        {"sessionId": "sess-1", "cwd": str(repo)},
    ])
    _fresh_mtime(transcript)

    signals = scan(claude_dir, _make_config([tmp_path]))

    assert len(signals) == 1
    root, signal = next(iter(signals.items()))
    assert Path(root) == repo
    assert signal.session_id == "sess-1"
    assert signal.agent == "claude-code"
    assert signal.raw_cwd == str(repo)


# ---------------------------------------------------------------------------
# 2. Directory name is ignored — we use the file's content, not its parent
# ---------------------------------------------------------------------------

def test_dir_name_is_ignored(tmp_path: Path) -> None:
    # The transcript sits in a dir that *looks* like a project slug but we'll
    # put an entirely different cwd in the file's content — the sensor must
    # go with the content.
    repo_a = tmp_path / "real-repo"
    repo_a.mkdir()

    fake_slug_dir = tmp_path / "claude_projects" / "-Users-me-fake-project-"
    fake_slug_dir.mkdir(parents=True)
    transcript = fake_slug_dir / "sess-x.jsonl"

    _write_lines(transcript, [
        {"sessionId": "sid-2", "cwd": str(repo_a)},
    ])
    _fresh_mtime(transcript)

    signals = scan(tmp_path / "claude_projects", _make_config([tmp_path]))

    assert len(signals) == 1
    root = next(iter(signals.keys()))
    assert Path(root) == repo_a  # content cwd wins, NOT the slug


# ---------------------------------------------------------------------------
# 3. cwd changing mid-file → last one wins
# ---------------------------------------------------------------------------

def test_cwd_changes_mid_file_last_wins(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    sub = repo / "packages" / "core"
    repo.mkdir()
    sub.mkdir(parents=True)

    claude_dir = tmp_path / "claude_projects"
    proj_dir = claude_dir / "anything"
    proj_dir.mkdir(parents=True)
    transcript = proj_dir / "sess-3.jsonl"

    _write_lines(transcript, [
        {"sessionId": "sid-3", "cwd": str(repo)},          # first cwd
        {"someOtherKey": "whatever"},                       # no cwd here
        {"message": "hi", "cwd": str(sub)},                 # LAST cwd wins
    ])
    _fresh_mtime(transcript)

    signals = scan(claude_dir, _make_config([tmp_path]))
    _, signal = next(iter(signals.items()))

    assert signal.raw_cwd == str(sub)
    assert signal.session_id == "sid-3"


# ---------------------------------------------------------------------------
# 4. Monorepo collapse — subdir resolves to the repo root
# ---------------------------------------------------------------------------

def test_monorepo_collapse_to_repo(tmp_path: Path) -> None:
    repo = tmp_path / "monorepo"
    packages_core = repo / "packages" / "core"
    repo.mkdir()
    (repo / ".git").mkdir()  # make it a real git repo for resolve_root
    packages_core.mkdir(parents=True)

    claude_dir = tmp_path / "claude_projects"
    proj_dir = claude_dir / "does-not-matter"
    proj_dir.mkdir(parents=True)
    transcript = proj_dir / "sess-4.jsonl"

    _write_lines(transcript, [
        {"sessionId": "sess-monorepo", "cwd": str(packages_core)},
    ])
    _fresh_mtime(transcript)

    signals = scan(claude_dir, _make_config([tmp_path]))

    assert len(signals) == 1
    root = next(iter(signals.keys()))
    assert Path(root) == repo


# ---------------------------------------------------------------------------
# 5. Truncated final line is normal and the signal still comes through
# ---------------------------------------------------------------------------

def test_truncated_final_line_does_not_abort(tmp_path: Path) -> None:
    repo = tmp_path / "repo-trunc"
    repo.mkdir()

    claude_dir = tmp_path / "claude_projects"
    proj_dir = claude_dir / "X"
    proj_dir.mkdir(parents=True)
    transcript = proj_dir / "sess-5.jsonl"

    # Build the file bytes by hand so we control exactly where truncation lands.
    complete_lines = [
        json.dumps({"sessionId": "sess-5", "cwd": str(repo)}),
        json.dumps({"message": "all good so far"}),
    ]
    truncated = '{"partial": true, "still_open":'  # unterminated JSON
    raw_text = "\n".join(complete_lines) + "\n" + truncated + "\n"
    transcript.write_text(raw_text, encoding="utf-8")
    _fresh_mtime(transcript)

    signals = scan(claude_dir, _make_config([tmp_path]))

    assert len(signals) == 1
    _, signal = next(iter(signals.items()))
    assert signal.session_id == "sess-5"
    assert Path(signal.raw_cwd) == repo


# ---------------------------------------------------------------------------
# 6. Cold file (mtime older than cold_cutoff_hours) is skipped
# ---------------------------------------------------------------------------

def test_cold_file_is_skipped(tmp_path: Path) -> None:
    repo = tmp_path / "cold-repo"
    repo.mkdir()

    claude_dir = tmp_path / "claude_projects"
    proj_dir = claude_dir / "Y"
    proj_dir.mkdir(parents=True)
    transcript = proj_dir / "cold.jsonl"

    _write_lines(transcript, [{"sessionId": "old-sess", "cwd": str(repo)}])

    # Pin mtime to ~83 days ago — well past the 60-day (1440h) default cutoff.
    old_ts = time.time() - (83 * 24 * 3600)
    os.utime(transcript, (old_ts, old_ts))

    signals = scan(claude_dir, _make_config([tmp_path]))
    assert signals == {}


# ---------------------------------------------------------------------------
# 7. Two transcripts resolving to the same root — newest ``at`` wins
# ---------------------------------------------------------------------------

def test_same_root_newest_at_wins(tmp_path: Path) -> None:
    repo = tmp_path / "shared-repo"
    repo.mkdir()

    claude_dir = tmp_path / "claude_projects"

    old_proj = claude_dir / "old-project-dir"
    old_proj.mkdir(parents=True)
    old_transcript = old_proj / "old-sess.jsonl"

    new_proj = claude_dir / "new-project-dir"
    new_proj.mkdir(parents=True)
    new_transcript = new_proj / "new-sess.jsonl"

    _write_lines(old_transcript, [
        {"sessionId": "old-sess", "cwd": str(repo)},
    ])
    _write_lines(new_transcript, [
        {"sessionId": "new-sess", "cwd": str(repo)},
    ])

    # Pin the new transcript to a *more recent* time than the old one.
    _fresh_mtime(new_transcript)
    old_ts = time.time() - (2 * 3600)  # 2 hours ago
    os.utime(old_transcript, (old_ts, old_ts))

    signals = scan(claude_dir, _make_config([tmp_path]))

    # Only one project root should appear, with the NEW session.
    assert len(signals) == 1
    _, signal = next(iter(signals.items()))
    assert signal.session_id == "new-sess"


# ---------------------------------------------------------------------------
# 8. Empty .jsonl and a directory with no .jsonl files → no signals, no raise
# ---------------------------------------------------------------------------

def test_empty_jsonl_and_no_jsonl_dirs(tmp_path: Path) -> None:
    claude_dir = tmp_path / "claude_projects"
    empty_dir = claude_dir / "empty-dir"
    empty_dir.mkdir(parents=True)
    (empty_dir / "nothing.jsonl").touch()  # zero bytes

    bare_dir = claude_dir / "no-sessions"
    bare_dir.mkdir()
    (bare_dir / "README.md").write_text("hi")  # only a .md, never queried

    signals = scan(claude_dir, _make_config([tmp_path]))
    assert signals == {}


# ---------------------------------------------------------------------------
# 9. agent is always "claude-code" and at is tz-aware (UTC)
# ---------------------------------------------------------------------------

def test_agent_and_tz_are_correct(tmp_path: Path) -> None:
    repo = tmp_path / "tz-repo"
    repo.mkdir()

    claude_dir = tmp_path / "claude_projects"
    proj_dir = claude_dir / "z"
    proj_dir.mkdir(parents=True)
    transcript = proj_dir / "sess-9.jsonl"
    _write_lines(transcript, [{"sessionId": "sid-9", "cwd": str(repo)}])
    _fresh_mtime(transcript)

    signals = scan(claude_dir, _make_config([tmp_path]))

    _, signal = next(iter(signals.items()))
    assert signal.agent == "claude-code"

    # ``at`` is built with tz=timezone.utc in the scanner; verify it has a
    # tzinfo set and is actually UTC — that's the invariant we promise callers.
    assert signal.at.tzinfo is not None, "at must be tz-aware"
    assert str(signal.at.tzinfo) == "UTC" or signal.at.utcoffset().total_seconds() == 0


# ---------------------------------------------------------------------------
# Cold cutoff boundary — explicit kwarg
# ---------------------------------------------------------------------------

def test_cold_cutoff_kwarg_skips_files(tmp_path: Path) -> None:
    """The cutoff is a kwarg default, so any non-default value should still gate."""
    repo = tmp_path / "cc-repo"
    repo.mkdir()

    claude_dir = tmp_path / "claude_projects"
    proj_dir = claude_dir / "w"
    proj_dir.mkdir(parents=True)
    transcript = proj_dir / "sess-w.jsonl"
    _write_lines(transcript, [{"sessionId": "sid-w", "cwd": str(repo)}])
    _fresh_mtime(transcript)

    # Set the file to 30 days ago. With the default cutoff (60 days) it would
    # still be scanned; with a 14-day cutoff it must be skipped.
    old_ts = time.time() - (30 * 24 * 3600)
    os.utime(transcript, (old_ts, old_ts))

    # Default cutoff (1440 h = 60 d): file is recent enough, signal appears.
    with_signals = scan(
        claude_dir, _make_config([tmp_path]), cold_cutoff_hours=1440
    )
    assert len(with_signals) == 1

    # Stricter cutoff (720 h = 30 d): file is older, no signal.
    without_signals = scan(
        claude_dir, _make_config([tmp_path]), cold_cutoff_hours=720
    )
    assert without_signals == {}


# ---------------------------------------------------------------------------
# Degradation tests (added by the orchestrator)
#
# As delegated, scan() raised FileNotFoundError on a missing claude_dir — a
# crash on any machine where Claude Code has never run, violating the
# "sensors degrade, never abort" invariant in CLAUDE.md.
# ---------------------------------------------------------------------------


def test_scan_missing_directory_returns_empty_not_raises(tmp_path):
    """A fresh machine has no ~/.claude/projects; that is not an error."""
    cfg = Config(roots=(tmp_path,))
    assert scan(tmp_path / "definitely-absent", cfg) == {}


def test_scan_empty_directory_returns_empty(tmp_path):
    """An existing but empty projects dir yields no signals."""
    empty = tmp_path / "projects"
    empty.mkdir()
    cfg = Config(roots=(tmp_path,))
    assert scan(empty, cfg) == {}


def test_scan_ignores_non_jsonl_and_stray_files(tmp_path):
    """Loose files beside the project dirs must not break the walk."""
    projects = tmp_path / "projects"
    projects.mkdir()
    (projects / "stray.txt").write_text("not a transcript")
    proj = projects / "-Users-me-thing"
    proj.mkdir()
    (proj / "notes.md").write_text("also not a transcript")

    cfg = Config(roots=(tmp_path,))
    assert scan(projects, cfg) == {}
