"""Tests for src/petridish/git."""

from __future__ import annotations

import subprocess
from pathlib import Path
from datetime import timezone

import pytest

from petridish.git import scan
from petridish.schema import GitState


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _git(cwd: str, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *args],
        check=False,
        capture_output=True,
        text=True,
        cwd=cwd,
        env={**__import__("os").environ},
    )


def _init(repo: str) -> subprocess.CompletedProcess[str]:
    # `git init <repo>` is unusual: cwd goes where we want the .git to land.
    return _git(".", "init", repo)


def _commit(
    repo: str,
    message: str = "initial",
    author_email: str = "t@e.st",
    author_name: str = "Test User",
    files: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    """Stage and commit. ``files`` writes content before adding, if given."""
    if files:
        import os
        for rel, content in files.items():
            full = os.path.join(repo, rel)
            os.makedirs(os.path.dirname(full), exist_ok=True)
            with open(full, "w") as fh:
                fh.write(content)

    add = _git(repo, "add", "-A")
    return subprocess.run(
        [
            "git", "-c", f"user.email={author_email}",
            "-c", f"user.name={author_name}",
            "commit", "--no-gpg-sign", "-m", message,
        ],
        check=False, capture_output=True, text=True, cwd=repo,
    )


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

def test_non_repo_returns_is_false(tmp_path):
    missing = str(tmp_path / "nope")
    state = scan(missing)
    assert isinstance(state, GitState)
    assert state.is_repo is False
    # Defaults for the rest.
    assert state.branch is None
    assert state.is_dirty is False


def test_fresh_one_commit_is_clean(tmp_path):
    repo = str(tmp_path / "r")
    _init(repo)
    _commit(repo, files={"README.md": "# hi"})

    state = scan(repo)
    assert state.is_repo is True
    assert state.branch == "master" or state.branch == "main"
    assert state.is_dirty is False
    assert state.uncommitted_files == 0
    assert state.last_commit_at is not None


def test_unmodified_file_sets_dirty(tmp_path):
    repo = str(tmp_path / "r")
    _init(repo)
    _commit(repo, files={"a.txt": "hello"})

    # Now modify one file so status --porcelain has one line.
    import os
    with open(os.path.join(repo, "a.txt"), "w") as fh:
        fh.write("world")

    state = scan(repo)
    assert state.is_dirty is True
    assert state.uncommitted_files == 1


def test_untracked_file_counts_as_uncommitted(tmp_path):
    repo = str(tmp_path / "r")
    _init(repo)
    _commit(repo, files={"a.txt": "hello"})

    import os
    with open(os.path.join(repo, "b.txt"), "w") as fh:
        fh.write("new file")

    state = scan(repo)
    assert state.is_dirty is True
    assert state.uncommitted_files == 1


def test_last_commit_at_is_timezone_aware(tmp_path):
    repo = str(tmp_path / "r")
    _init(repo)
    _commit(repo, files={"x": "1"})

    state = scan(repo)
    assert state.last_commit_at is not None
    assert state.last_commit_at.tzinfo is not None
    # The zone offset should match UTC-ish (git defaults to TZ of committer).
    assert state.last_commit_at.utcoffset() is not None


def test_empty_repo_no_crash(tmp_path):
    repo = str(tmp_path / "r")
    _init(repo)  # No commits.

    state = scan(repo)
    assert state.is_repo is True
    # HEAD exists but no commits -> last_commit_at None, no TypeError raised.
    assert state.last_commit_at is None


def test_author_filter_sets_mine_last_commit(tmp_path):
    repo = str(tmp_path / "r")
    _init(repo)
    _commit(repo, author_email="alice@example.com", files={"k": "v"})

    state = scan(repo, author_patterns=("alice@example.com",))
    assert state.mine_last_commit_at is not None
    assert state.last_commit_at == state.mine_last_commit_at


def test_author_filter_excludes_non_matching(tmp_path):
    repo = str(tmp_path / "r")
    _init(repo)
    _commit(repo, author_email="bob@example.com", files={"k": "v"})

    state = scan(repo, author_patterns=("charlie@example.com",))
    assert state.mine_last_commit_at is None


def test_github_url_from_ssh(tmp_path):
    repo = str(tmp_path / "r")
    _init(repo)
    _git(repo, "remote", "add", "origin", "git@github.com:user/repo.git")
    _commit(repo, files={"x": "1"})

    state = scan(repo)
    assert state.github_url == "https://github.com/user/repo"


def test_github_url_from_https(tmp_path):
    repo = str(tmp_path / "r")
    _init(repo)
    _git(
        repo, "remote", "add", "origin",
        "https://github.com/user/repo.git",
    )
    _commit(repo, files={"x": "1"})

    state = scan(repo)
    assert state.github_url == "https://github.com/user/repo"


def test_non_github_remote_yields_none(tmp_path):
    repo = str(tmp_path / "r")
    _init(repo)
    _git(repo, "remote", "add", "origin", "https://gitlab.com/user/repo.git")
    _commit(repo, files={"x": "1"})

    state = scan(repo)
    assert state.github_url is None


def test_no_remote_yields_none(tmp_path):
    repo = str(tmp_path / "r")
    _init(repo)
    _commit(repo, files={"x": "1"})

    state = scan(repo)
    assert state.github_url is None


def test_detach_head_stores_HEAD(tmp_path):
    repo = str(tmp_path / "r")
    _init(repo)
    _commit(repo, files={"x": "1"})
    sha_out = _git(repo, "rev-parse", "HEAD")
    assert sha_out.returncode == 0
    _git(repo, "checkout", "--detach", sha_out.stdout.strip())

    state = scan(repo)
    assert state.is_repo is True
    assert state.branch == "HEAD"


def test_scan_uses_portable_dataclass_replace():
    """Guard the 3.12 floor declared in pyproject.

    As delegated, scan() used ``instance.__replace__(...)``, which only exists
    on Python 3.13+. The suite passed on 3.14 while the package claimed
    ``requires-python = ">=3.12"``, so this would have failed only on the
    oldest supported interpreter — the one least likely to be tested here.
    """
    source = (Path(__file__).parent.parent / "src" / "petridish" / "git.py").read_text()
    assert "__replace__" not in source
