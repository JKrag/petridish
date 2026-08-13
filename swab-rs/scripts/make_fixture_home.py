#!/usr/bin/env python3
"""Builds a small, hermetic fixture $HOME for the Python-vs-Rust differential oracle.

Layout mirrors the fixture shape used by tests/test_scan.py (real git repos with pinned
author/date env vars, a synthetic ~/.claude/projects transcript, a synthetic VS Code
workspaceStorage tree) — deliberately real files/processes, not mocks, per CLAUDE.md's
testing philosophy. Prints the fixture HOME path on stdout; nothing else.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path


def _git(args: list[str], cwd: Path, env: dict[str, str] | None = None) -> None:
    full_env = os.environ.copy()
    if env:
        full_env.update(env)
    subprocess.run(["git", *args], cwd=str(cwd), check=True, capture_output=True, env=full_env)


def _init_repo(path: Path, *, dirty: bool = False) -> None:
    path.mkdir(parents=True, exist_ok=True)
    _git(["init", "-q"], path)
    _git(["config", "user.email", "test@none.invalid"], path)
    _git(["config", "user.name", "Test"], path)
    (path / "file.txt").write_text("hi\n")
    _git(["add", "-A"], path)
    _git(
        ["commit", "-q", "-m", "initial commit"],
        path,
        env={
            "GIT_AUTHOR_DATE": "2026-08-01T12:00:00",
            "GIT_COMMITTER_DATE": "2026-08-01T12:00:00",
        },
    )
    if dirty:
        (path / "scratch.txt").write_text("uncommitted\n")


def build(root: Path) -> Path:
    home = root / "fake_home"
    (home / ".claude" / "projects").mkdir(parents=True, exist_ok=True)
    (home / "Library" / "Application Support" / "Code" / "User" / "workspaceStorage").mkdir(
        parents=True, exist_ok=True
    )
    (home / ".petridish").mkdir(parents=True, exist_ok=True)

    # Under $HOME/repos so the default config.roots crawl (["~/repos", "~/learning"])
    # discovers both repos, not just the one with an agent signal pointing at it.
    repos_dir = home / "repos"
    repos_dir.mkdir(parents=True, exist_ok=True)

    clean_repo = repos_dir / "clean-repo"
    _init_repo(clean_repo, dirty=False)

    dirty_repo = repos_dir / "dirty-repo"
    _init_repo(dirty_repo, dirty=True)

    # Synthetic Claude Code transcript: one project dir, one session, cwd matches clean_repo.
    slug_dir = home / ".claude" / "projects" / "-fixture-slug"
    slug_dir.mkdir(parents=True, exist_ok=True)
    transcript = slug_dir / "session-1.jsonl"
    lines = [
        {"sessionId": "fixture-sess-1", "cwd": str(clean_repo)},
        {"message": "still working", "cwd": str(clean_repo)},
    ]
    transcript.write_text("\n".join(json.dumps(line) for line in lines) + "\n")

    # No VS Code Copilot data and no events.ndjson in the base fixture — both sensors
    # must degrade to empty, never error, on an all-absent input (invariant #5).

    return home


if __name__ == "__main__":
    target = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(os.environ["TMPDIR"]) / "swab-rs-fixture"
    if target.exists():
        import shutil

        shutil.rmtree(target)
    target.mkdir(parents=True)
    home = build(target)
    print(str(home))
