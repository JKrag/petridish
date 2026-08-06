"""Tests for ``radar.sensors.copilot``.

Every test builds fixtures on ``tmp_path``, never touching the real
``~/Library/Application Support/Code/...`` tree.  The sensor degrades when
files are missing or unreadable, so malformed JSON and missing directories
must not raise — the expected value is an empty mapping (or fewer signals
than the test otherwise produced).
"""

from __future__ import annotations

import json
import os
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

import pytest

from radar.config import Config
from radar.schema import AgentSignal
from radar.sensors.copilot import scan


def _config() -> Config:
    """Lightweight Config for tests.

    Empty roots keeps resolve_root's ceiling at home + filesystem root — the
    absolute paths under ``tmp_path`` always walk up to ``/`` without hitting a
    root that would shortcut the walk.  That keeps fixture paths stable across
    machines (``tmp_path`` is under ``/private/tmp`` regardless of who runs the
    test).
    """
    return Config(roots=(), extra_paths=())


def _build_workspace(
    tmp_path: Path,
    folder_uri: str,
    *,
    extra_files: list[os.PathLike] | None = None,
    malformed_json: bool = False,
    multi_root: bool = False,
) -> tuple[Path, float]:
    """Build a single hash dir under ``tmp_path`` and return ``(hash_dir, newest)``.

    ``newest`` is the mtime of the single chat file we always write so the
    caller can synchronise expectations against :func:`scan`'s ``at`` value.
    """
    hash_dir = tmp_path / "abc123def456"
    hash_dir.mkdir()
    chat_dir = hash_dir / "chatSessions"
    chat_dir.mkdir()

    if not malformed_json and not multi_root:
        ws = {"folder": folder_uri}
    elif multi_root:
        # v1 does not support multi-root workspaces — they must be skipped.
        ws = {"workspace": [{"name": "a", "folderUri": folder_uri}]}
    else:
        ws = "this is not json {"

    (hash_dir / "workspace.json").write_text(
        json.dumps(ws), encoding="utf-8"
    )

    # Always write a chat file so we have a mtime to correlate against.
    chat_file = chat_dir / "some-session.jsonl"
    chat_file.write_text("one line\n")

    newest = float(os.path.getmtime(chat_file))
    if extra_files:
        # Make sure the chat file remains the newest.
        for name in extra_files:
            p = chat_dir / name
            p.write_text("")
            os.utime(p, (newest, newest))

    return hash_dir, newest


def _folder_uri(path: Path, label: str = "proj") -> str:
    """Turn a local path into the ``file://`` URI vs-code uses in workspace.json."""
    return (path / label).as_uri()


# ---------------------------------------------------------------------------
# 1. A well-formed hash produces one signal with agent == "copilot".
# ---------------------------------------------------------------------------

def test_one_hash_yields_one_signal(tmp_path: Path) -> None:
    config = _config()
    folder_path = tmp_path / "proj"
    folder_path.mkdir()

    uri = _folder_uri(tmp_path, "proj")
    _build_workspace(tmp_path, uri)

    signals = scan(tmp_path, config)

    assert len(signals) == 1
    root, sig = next(iter(signals.items()))
    assert root == str(folder_path.resolve())
    assert sig.agent == "copilot"
    assert sig.session_id is None
    assert sig.event is None
    assert sig.raw_cwd == str(folder_path.resolve())


# ---------------------------------------------------------------------------
# 2. A hash missing chatSessions/ is skipped.
# ---------------------------------------------------------------------------

def test_missing_chat_sessions_skips_hash(tmp_path: Path) -> None:
    config = _config()
    folder_path = tmp_path / "proj"
    folder_path.mkdir()
    uri = urllib.request.pathname2url(str(folder_path))

    hash_dir = tmp_path / "nochat"
    hash_dir.mkdir()
    (hash_dir / "workspace.json").write_text(
        json.dumps({"folder": uri}), encoding="utf-8"
    )
    # No chatSessions/ at all.

    signals = scan(tmp_path, config)
    assert signals == {}


# ---------------------------------------------------------------------------
# 3. A hash missing workspace.json is skipped.
# ---------------------------------------------------------------------------

def test_missing_workspace_json_skips_hash(tmp_path: Path) -> None:
    config = _config()
    folder_path = tmp_path / "proj"
    folder_path.mkdir()

    hash_dir = tmp_path / "nojson"
    hash_dir.mkdir()
    (hash_dir / "chatSessions").mkdir()
    (hash_dir / "chatSessions" / "s.jsonl").write_text("")

    signals = scan(tmp_path, config)
    assert signals == {}


# ---------------------------------------------------------------------------
# 4. Multi-root ("workspace") instead of single-folder is skipped.
# ---------------------------------------------------------------------------

def test_multi_root_workspaces_skipped(tmp_path: Path) -> None:
    config = _config()
    folder_path = tmp_path / "proj"
    folder_path.mkdir()

    # Same folder for both keys just to keep the fixture simple — what
    # matters is that "folder" is absent from the dict.
    uri = urllib.request.pathname2url(str(folder_path))
    hash_dir = tmp_path / "multi"
    hash_dir.mkdir()
    (hash_dir / "workspace.json").write_text(
        json.dumps({"workspace": [{"name": "a", "folderUri": uri}]}),
        encoding="utf-8",
    )
    (hash_dir / "chatSessions").mkdir()
    (hash_dir / "chatSessions" / "s.jsonl").write_text("")

    signals = scan(tmp_path, config)
    assert signals == {}


# ---------------------------------------------------------------------------
# 5. Malformed JSON is skipped, not raised.
# ---------------------------------------------------------------------------

def test_malformed_json_skips_without_raising(tmp_path: Path) -> None:
    config = _config()
    folder_path = tmp_path / "proj"
    folder_path.mkdir()
    uri = urllib.request.pathname2url(str(folder_path))

    hash_dir = tmp_path / "broken"
    hash_dir.mkdir()
    (hash_dir / "workspace.json").write_text("this is not json {", encoding="utf-8")
    (hash_dir / "chatSessions").mkdir()
    (hash_dir / "chatSessions" / "s.jsonl").write_text("")

    # Should not raise; must return an empty mapping since the hash failed.
    assert scan(tmp_path, config) == {}


# ---------------------------------------------------------------------------
# 6. A folder URI with a space resolves to a real-space path.
# ---------------------------------------------------------------------------

def test_folder_uri_with_space_resolves_to_real_space(tmp_path: Path) -> None:
    config = _config()
    # This directory name contains an ASCII space — the *only* thing that
    # tests the url2pathname-vs-slicing requirement.
    folder_name = "My Project"
    folder_path = tmp_path / folder_name
    folder_path.mkdir()

    # Build the URI with an explicit %20 escape in the path component — this
    # is what Path.as_uri() (and VS Code) produce on posix.  We want
    # url2pathname to decode the %20 back into a real space, which is why we
    # delegate to url2pathname instead of slicing ``file://`` off by hand.
    uri = (tmp_path / folder_name).as_uri()

    _build_workspace(tmp_path, uri)
    signals = scan(tmp_path, config)

    assert len(signals) == 1
    _, sig = next(iter(signals.items()))
    # The resolved path is what *urllib.request.url2pathname* would return
    # for the decoded path — a real space, not "%20".
    assert "%" not in sig.raw_cwd
    assert " " in sig.raw_cwd
    # And the path must exist on disk (url2pathname shouldn't have lost
    # anything during decoding).
    assert sig.raw_cwd == str(folder_path.resolve())


# ---------------------------------------------------------------------------
# 7. The signal's ``at`` equals the newest mtime in chatSessions/.
# ---------------------------------------------------------------------------

def test_at_equals_newest_chat_session_mtime(tmp_path: Path) -> None:
    config = _config()
    folder_path = tmp_path / "proj"
    folder_path.mkdir()
    uri = _folder_uri(tmp_path, "proj")

    hash_dir = tmp_path / "mtime"
    hash_dir.mkdir()
    (hash_dir / "workspace.json").write_text(
        json.dumps({"folder": uri}), encoding="utf-8"
    )
    chat_dir = hash_dir / "chatSessions"
    chat_dir.mkdir()

    # Three files with distinct, strictly increasing mtimes.  The newest one
    # must win.
    new = time.time()
    (chat_dir / "a.jsonl").write_text("old")
    os.utime(chat_dir / "a.jsonl", (new - 100, new - 100))

    (chat_dir / "b.jsonl").write_text("mid")
    os.utime(chat_dir / "b.jsonl", (new - 50, new - 50))

    (chat_dir / "c.jsonl").write_text("new")
    os.utime(chat_dir / "c.jsonl", (new, new))

    # And a *subdirectory* sitting inside chatSessions/ so is_file() filters
    # it out and we're actually measuring files, not directory mtimes.
    (chat_dir / "sub").mkdir()

    signals = scan(tmp_path, config)
    assert len(signals) == 1
    _, sig = next(iter(signals.items()))

    assert sig.root == str(folder_path.resolve())
    # ``at`` is tz-aware UTC second-resolution — match against the float
    # using ``fromtimestamp``, then compare.
    expected_at = datetime.fromtimestamp(new, tz=timezone.utc)
    assert sig.at == expected_at


# ---------------------------------------------------------------------------
# 8. A hash whose newest mtime is older than cold_cutoff_hours is skipped.
# ---------------------------------------------------------------------------

def test_hot_hash_skips_cold_hash(tmp_path: Path) -> None:
    config = _config()
    hot_folder = tmp_path / "hot_proj"
    hot_folder.mkdir()
    cold_folder = tmp_path / "cold_proj"
    cold_folder.mkdir()

    hot_uri = _folder_uri(tmp_path, "hot_proj")
    cold_uri = _folder_uri(tmp_path, "cold_proj")

    # Hot hash: a single file right now.
    hot_dir = tmp_path / "hot"
    hot_dir.mkdir()
    (hot_dir / "workspace.json").write_text(
        json.dumps({"folder": hot_uri}), encoding="utf-8"
    )
    chat = hot_dir / "chatSessions"
    chat.mkdir()
    now = time.time()
    (chat / "recent.jsonl").write_text("hi")
    os.utime(chat / "recent.jsonl", (now, now))

    # Cold hash: 80 hours old — well past the default 1440h cutoff.
    cold_dir = tmp_path / "cold"
    cold_dir.mkdir()
    (cold_dir / "workspace.json").write_text(
        json.dumps({"folder": cold_uri}), encoding="utf-8"
    )
    cchat = cold_dir / "chatSessions"
    cchat.mkdir()
    old = now - 80 * 3600
    (cchat / "old.jsonl").write_text("hi")
    os.utime(cchat / "old.jsonl", (old, old))

    signals = scan(tmp_path, config, cold_cutoff_hours=1)
    assert list(signals) == [str(hot_folder.resolve())]
    assert next(iter(signals.values())).agent == "copilot"


# ---------------------------------------------------------------------------
# 9. A missing workspace_storage_dir returns {} without raising.
# ---------------------------------------------------------------------------

def test_missing_workspace_storage_returns_empty(tmp_path: Path) -> None:
    config = _config()
    signals = scan(tmp_path / "does_not_exist", config)
    assert signals == {}


# ---------------------------------------------------------------------------
# 10. Two hashes pointing at the same folder collapse to one entry (newest).
# ---------------------------------------------------------------------------

def test_two_hashes_same_folder_collapse_newest_wins(tmp_path: Path) -> None:
    config = _config()
    folder_path = tmp_path / "proj"
    folder_path.mkdir()
    uri = _folder_uri(tmp_path, "proj")

    # Hot hash: file right now.
    hot_dir = tmp_path / "hash1"
    hot_dir.mkdir()
    (hot_dir / "workspace.json").write_text(
        json.dumps({"folder": uri}), encoding="utf-8"
    )
    hchat = hot_dir / "chatSessions"
    hchat.mkdir()
    now = time.time()
    (hchat / "hot.jsonl").write_text("hi")
    os.utime(hchat / "hot.jsonl", (now, now))

    # Cold hash: older, but same folder — must collapse, newest wins.
    cold_dir = tmp_path / "hash2"
    cold_dir.mkdir()
    (cold_dir / "workspace.json").write_text(
        json.dumps({"folder": uri}), encoding="utf-8"
    )
    cchat = cold_dir / "chatSessions"
    cchat.mkdir()
    old = now - 72 * 3600
    (cchat / "cold.jsonl").write_text("hi")
    os.utime(cchat / "cold.jsonl", (old, old))

    signals = scan(tmp_path, config)
    assert set(signals) == {str(folder_path.resolve())}
    _, sig = next(iter(signals.items()))
    # The hot hash's mtime must win — it's newer.
    assert sig.at == datetime.fromtimestamp(now, tz=timezone.utc)
