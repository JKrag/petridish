#!/usr/bin/env python3
"""Builds the fixture $HOME for the Python-vs-Rust differential oracle (diff_check.sh).

Layout mirrors the fixture shapes used across tests/test_*.py (real git repos with pinned
author/date env vars, synthetic ~/.claude/projects transcripts, a synthetic VS Code
workspaceStorage tree, events.ndjson, last-status.json) — deliberately real files/processes,
not mocks, per CLAUDE.md's testing philosophy. Prints the fixture HOME path on stdout;
nothing else.

Coverage checklist (each line exercises a field diff_check.sh's mask can't hide a bug in):
  - author matching the default "Jan.*Krag" pattern -> is_foreign=false, mine_last_commit_at set
  - author NOT matching -> is_foreign=true (unless dirty, which overrides to not-foreign)
  - empty repo (0 commits) -> no crash, last_commit_at=None
  - SSH remote, HTTPS remote, no remote -> github_url normalization / None
  - monorepo: transcript cwd changes mid-file, last value (a subdir) wins, resolve_root
    collapses the subdir signal onto the repo root, not a phantom project
  - truncated trailing JSONL line in a transcript -> skipped, falls back to previous line
  - VS Code Copilot workspace with a %20-encoded folder URI -> percent-decoded, not sliced
  - events.ndjson with one hook-fast-path signal
  - last-status.json (quota) with a full rate_limits + context_window payload

All commit/mtime timestamps are pinned well away from the bucketing/liveness boundaries
(90s / 1800s working-recent, 48h/336h/1440h status buckets) so a few seconds of wall-clock
skew between the Python and Rust runs can never flip a bucket and produce a false mismatch.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path
from urllib.parse import quote


def _git(args: list[str], cwd: Path, env: dict[str, str] | None = None) -> None:
    full_env = os.environ.copy()
    if env:
        full_env.update(env)
    subprocess.run(["git", *args], cwd=str(cwd), check=True, capture_output=True, env=full_env)


def _init_repo(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=True)
    _git(["init", "-q"], path)
    _git(["config", "user.email", "test@none.invalid"], path)
    _git(["config", "user.name", "Test"], path)


def _commit(
    path: Path,
    *,
    message: str = "initial commit",
    author_name: str = "Test",
    author_email: str = "test@none.invalid",
    date: str = "2026-08-01T12:00:00",
    filename: str = "file.txt",
) -> None:
    (path / filename).write_text("hi\n")
    _git(["add", "-A"], path)
    _git(
        ["commit", "-q", "-m", message, f"--author={author_name} <{author_email}>"],
        path,
        env={"GIT_AUTHOR_DATE": date, "GIT_COMMITTER_DATE": date},
    )


def _dirty(path: Path, filename: str = "scratch.txt") -> None:
    (path / filename).write_text("uncommitted\n")


def build(root: Path) -> Path:
    home = root / "fake_home"
    (home / ".claude" / "projects").mkdir(parents=True, exist_ok=True)
    (home / "Library" / "Application Support" / "Code" / "User" / "workspaceStorage").mkdir(
        parents=True, exist_ok=True
    )
    (home / ".petridish").mkdir(parents=True, exist_ok=True)

    # Under $HOME/repos so the default config.roots crawl (["~/repos", "~/learning"])
    # discovers everything below, not just paths with an agent signal pointing at them.
    repos = home / "repos"
    repos.mkdir(parents=True, exist_ok=True)

    # 1. Foreign author, clean tree, no remote. Also gets an agent signal (below).
    clean_repo = repos / "clean-repo"
    _init_repo(clean_repo)
    _commit(clean_repo, date="2026-07-15T09:00:00")

    # 2. Foreign author, dirty tree (positive evidence -> not foreign despite author).
    dirty_repo = repos / "dirty-repo"
    _init_repo(dirty_repo)
    _commit(dirty_repo, date="2026-07-15T09:00:00")
    _dirty(dirty_repo)

    # 3. Author matches the default "Jan.*Krag" pattern -> is_foreign=false,
    #    mine_last_commit_at set. SSH remote -> github_url normalization.
    mine_repo = repos / "mine-repo"
    _init_repo(mine_repo)
    _commit(mine_repo, author_name="Jan Krag", author_email="jan@example.invalid", date="2026-07-20T09:00:00")
    _git(["remote", "add", "origin", "git@github.com:JKrag/mine-repo.git"], mine_repo)

    # 4. HTTPS remote -> same github_url normalization, different input form.
    https_repo = repos / "https-repo"
    _init_repo(https_repo)
    _commit(https_repo, author_name="Jan Krag", author_email="jan@example.invalid", date="2026-07-20T09:00:00")
    _git(["remote", "add", "origin", "https://github.com/JKrag/https-repo.git"], https_repo)

    # 5. Empty repo — zero commits, must not crash, last_commit_at stays None.
    empty_repo = repos / "empty-repo"
    _init_repo(empty_repo)

    # 6. Monorepo: root .git, agent signal points at a subdir mid-session, last cwd wins,
    #    and resolve_root must collapse the subdir signal onto this same root (not a
    #    separate phantom project).
    monorepo = repos / "monorepo"
    monorepo.mkdir(parents=True, exist_ok=True)
    _init_repo(monorepo)
    _commit(monorepo, date="2026-07-18T09:00:00")
    monorepo_sub = monorepo / "packages" / "core"
    monorepo_sub.mkdir(parents=True, exist_ok=True)

    # --- Claude Code transcripts (~/.claude/projects/<slug>/<session>.jsonl) ---
    # Slugs are deliberately arbitrary/non-decodable strings (F2) — never derived from the
    # real path, matching how Claude Code actually names these directories.
    projects_dir = home / ".claude" / "projects"

    # Liveness (agent.state / status_bucket) is derived from file mtime vs "now" at scan
    # time. The Python and Rust runs happen minutes apart (a cargo build sits in between),
    # so an mtime of "just now" risks straddling the 90s/1800s boundary and producing a
    # false mismatch. Pin transcript/chatSessions mtimes well inside the "recent" band
    # (5 minutes old) so both runs land in the same bucket regardless of build time.
    recent_mtime = time.time() - 300

    clean_slug = projects_dir / "-fixture-slug-clean"
    clean_slug.mkdir(parents=True, exist_ok=True)
    clean_transcript = clean_slug / "session-1.jsonl"
    clean_transcript.write_text(
        "\n".join(
            json.dumps(line)
            for line in [
                {"sessionId": "fixture-sess-clean", "cwd": str(clean_repo)},
                {"message": "still working", "cwd": str(clean_repo)},
            ]
        )
        + "\n"
    )
    os.utime(clean_transcript, (recent_mtime, recent_mtime))

    monorepo_slug = projects_dir / "-fixture-slug-monorepo"
    monorepo_slug.mkdir(parents=True, exist_ok=True)
    # cwd changes mid-file (root -> subdir); last value must win (F9); a truncated final
    # line must be skipped and fall back to the previous valid line (invariant #4).
    good_lines = [
        json.dumps({"sessionId": "fixture-sess-mono", "cwd": str(monorepo)}),
        json.dumps({"message": "moved into packages/core", "cwd": str(monorepo_sub)}),
    ]
    truncated_tail = '{"message": "mid-write when the daemon read thi'  # deliberately incomplete
    monorepo_transcript = monorepo_slug / "session-1.jsonl"
    monorepo_transcript.write_text("\n".join(good_lines) + "\n" + truncated_tail)
    os.utime(monorepo_transcript, (recent_mtime, recent_mtime))

    # --- VS Code Copilot: workspaceStorage/<hash>/{workspace.json,chatSessions/} ---
    # folder URI is %20-encoded (a real space in the path) to verify percent-decoding via
    # the `url` crate rather than naive string slicing.
    copilot_repo = repos / "My Project"
    _init_repo(copilot_repo)
    _commit(copilot_repo, date="2026-07-22T09:00:00")

    workspace_storage = (
        home / "Library" / "Application Support" / "Code" / "User" / "workspaceStorage"
    )
    hash_dir = workspace_storage / "fixturehash01"
    (hash_dir / "chatSessions").mkdir(parents=True, exist_ok=True)
    copilot_session = hash_dir / "chatSessions" / "session.json"
    copilot_session.write_text("{}\n")
    os.utime(copilot_session, (recent_mtime, recent_mtime))
    folder_uri = "file://" + quote(str(copilot_repo))
    (hash_dir / "workspace.json").write_text(json.dumps({"folder": folder_uri}) + "\n")

    # --- events.ndjson (swab-hook fast path) — a signal for a repo with no transcript. ---
    events_signal_repo = repos / "events-only-repo"
    _init_repo(events_signal_repo)
    _commit(events_signal_repo, date="2026-07-25T09:00:00")
    events_path = home / ".petridish" / "events.ndjson"
    events_path.write_text(
        json.dumps(
            {
                "cwd": str(events_signal_repo),
                "session_id": "fixture-hook-sess",
                "event": "PreToolUse",
                "at": "2026-08-05T09:00:00Z",
            }
        )
        + "\n"
    )

    # --- last-status.json (account-global quota) ---
    last_status = {
        "ts": "2026-08-05T09:00:00Z",
        "rate_limits": {
            "five_hour": {"used_percentage": 9, "resets_at": int(time.mktime((2026, 8, 5, 14, 0, 0, 0, 0, 0)))},
            "seven_day": {"used_percentage": 42, "resets_at": int(time.mktime((2026, 8, 10, 9, 0, 0, 0, 0, 0)))},
        },
        "context_window": {"used_percentage": 28},
    }
    (home / ".claude" / "last-status.json").write_text(json.dumps(last_status) + "\n")

    return home


if __name__ == "__main__":
    target = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(os.environ["TMPDIR"]) / "swab-rs-fixture"
    if target.exists():
        import shutil

        shutil.rmtree(target)
    target.mkdir(parents=True)
    home = build(target)
    print(str(home))
