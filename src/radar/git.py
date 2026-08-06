"""Git repository scanner.

Walks a path and returns a frozen ``GitState`` describing the repo (or the
empty-fallback state if the path is not a repo). Subprocess calls degrade the
affected field instead of raising.
"""

from __future__ import annotations

import subprocess
from dataclasses import replace
from datetime import datetime, timezone
from pathlib import Path
from typing import Union

from radar.schema import GitState


def _run(
    path: str,
    *args: str,
) -> subprocess.CompletedProcess[str] | None:
    """Run ``git -C <path> args`` with the project's standard timeout.

    Returns ``None`` when the call itself raises (timeouts, broken env), so
    callers can degrade gracefully.
    """
    try:
        return subprocess.run(
            ["git", "-C", path, *args],
            check=False,
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (subprocess.TimeoutExpired, OSError):
        return None


def _parse_date(text: str) -> datetime | None:
    """Parse an ISO date from ``git log``; ``None`` on any failure."""
    if not text:
        return None
    try:
        dt = datetime.fromisoformat(text.strip())
    except (ValueError, TypeError):
        return None
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt


def _github_url(remote: str | None) -> str | None:
    """Normalise ``git@...`` and ``https://...`` to a bare GitHub URL.

    Returns ``None`` for non-GitHub remotes or missing remotes.
    """
    if not remote:
        return None
    url = remote.strip().rstrip("/")
    cleaned: str | None

    if url.startswith("git@github.com:"):
        rest = url.removeprefix("git@github.com:")
        cleaned = "https://github.com/" + rest.removesuffix(".git")
    elif url.startswith("https://github.com/"):
        cleaned = url.removesuffix(".git")
    else:
        return None

    return cleaned


def scan(
    path: Union[str, Path],
    author_patterns: tuple[str, ...] = (),
    since: str = "3 years",
) -> GitState:
    """Inspect ``path`` and return a ``GitState`` describing the repo.

    A single failed subprocess degrades only that field; the rest of the
    ``GitState`` is still populated. A path that isn't a git repo returns the
    empty sentinel ``GitState(is_repo=False)`` immediately.
    """
    path = str(Path(path).expanduser())

    rev_parse = _run(path, "rev-parse", "--git-dir")
    if rev_parse is None or rev_parse.returncode != 0:
        return GitState(is_repo=False)

    result = GitState(is_repo=True)

    # Branch.
    branch_out = _run(path, "rev-parse", "--abbrev-ref", "HEAD")
    if branch_out is not None and branch_out.returncode == 0:
        raw = (branch_out.stdout or "").strip()
        if raw:
            result = replace(result, branch=raw)

    # Dirty state.
    status_out = _run(path, "status", "--porcelain")
    if status_out is not None and status_out.returncode == 0:
        lines = [l for l in (status_out.stdout or "").splitlines() if l.strip()]
        dirty = len(lines) > 0
        result = replace(result, is_dirty=dirty, uncommitted_files=len(lines))

    # Last commit.
    log_out = _run(path, "log", "-1", "--format=%cI")
    if log_out is not None and log_out.returncode == 0:
        last_dt = _parse_date(log_out.stdout)
        if last_dt is not None:
            result = replace(result, last_commit_at=last_dt)

    # Mine last commit.
    my_best: datetime | None = None
    for pattern in author_patterns:
        mine_out = _run(
            path, "log", "-1", "--format=%cI",
            f"--author={pattern}", f"--since={since}",
        )
        if mine_out is None or mine_out.returncode != 0:
            continue
        dt = _parse_date(mine_out.stdout)
        if dt is not None:
            if my_best is None or dt > my_best:
                my_best = dt
    if my_best is not None:
        result = replace(result, mine_last_commit_at=my_best)

    # Remote.
    remote_out = _run(path, "remote", "get-url", "origin")
    if remote_out is not None and remote_out.returncode == 0:
        url = _github_url(remote_out.stdout)
        if url is not None:
            result = replace(result, github_url=url)

    return result
