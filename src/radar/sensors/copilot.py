"""Copilot (VS Code) sensor.

Walks the VS Code ``workspaceStorage/<hash>/`` directory, picks out hashes that
contain *both* a ``workspace.json`` (with a single-folder ``"folder"`` value)
and a live ``chatSessions/`` directory, and emits one :class:`AgentSignal`
per resolved project root keyed by root.

Design notes that mirror real machine findings:

* ``chatSessions/`` may exist while ``workspace.json`` is absent (or vice
  versa) — both must be present, or the hash is a no-op.
* ``"folder"`` in ``workspace.json`` is a **file:// URI**.  It must be routed
  through ``urllib.parse`` + ``url2pathname``, not string-sliced, because
  paths can hold ``%20`` escapes and other percent-encoded sequences.
* Multi-root workspaces (keyed by ``"workspace"``) are skipped in v1.
* Cold activity (older than ``cold_cutoff_hours`` since the newest session
  file) is ignored so dashboards don't light up on dormant projects.
"""

from __future__ import annotations

import json
import os
import time
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

from radar.discovery import resolve_root
from radar.schema import AgentSignal


def _uri_to_path(uri: str) -> str:
    """Convert a ``file://`` URI to an absolute filesystem path.

    Splits the scheme off with :func:`urllib.parse.urlparse`, then hands the
    path component to :func:`urllib.request.url2pathname`, which decodes
    percent-escapes (``%20`` in a path with spaces) into real characters. That
    decoding is exactly why we must not slice ``file://`` off by hand.

    Deliberately does NOT use ``url2pathname(uri, require_scheme=True)``: that
    keyword was only added in Python 3.14, and this package declares
    ``requires-python = ">=3.12"``, so it would raise TypeError on the oldest
    interpreters we claim to support.
    """
    parsed = urllib.parse.urlparse(uri)
    if parsed.scheme and parsed.scheme != "file":
        raise ValueError(f"not a file:// URI: {uri!r}")
    return urllib.request.url2pathname(parsed.path)


def _read_chat_session_mtime(chat_dir: Path, hash_path: Path | None) -> float | None:
    """Return the newest mtime among files inside ``chat_dir``, or None.

    A directory containing no files (or that is not readable) produces
    ``None`` — the calling loop treats that as "this hash has nothing to
    offer", not an error.  Any read failure is caught here so one rotten
    hash can't abort the scan.
    """
    newest: float | None = None
    try:
        entries = list(chat_dir.iterdir())
    except (FileNotFoundError, NotADirectoryError, PermissionError):
        return None
    for entry in entries:
        if not entry.is_file():
            continue
        try:
            mt = entry.stat().st_mtime
        except (FileNotFoundError, NotADirectoryError, PermissionError):
            continue
        if newest is None or mt > newest:
            newest = mt
    return newest


def scan(
    workspace_storage_dir: str | os.PathLike,
    config,
    *,
    cold_cutoff_hours: float = 1440,
) -> dict[str, AgentSignal]:
    """Scan one VS Code ``workspaceStorage/`` tree for active Copilot projects.

    Returns :data:`{}` (never raises) when the storage directory is missing
    or unreadable — sensors degrade, they do not abort.  When no hashes are
    hot enough to matter, the empty mapping is equally valid.
    """
    root_dir = Path(workspace_storage_dir)
    if not root_dir.is_dir():
        return {}

    cutoff = time.time() - cold_cutoff_hours * 3600
    signals: dict[str, AgentSignal] = {}

    try:
        hashes = sorted(root_dir.iterdir())
    except (PermissionError, NotADirectoryError):
        return {}

    for hash_dir in hashes:
        if not hash_dir.is_dir():
            continue
        signals = _process_one_hash(hash_dir, config, signals, cutoff)

    return signals


def _process_one_hash(
    hash_dir: Path,
    config,
    signals: dict[str, AgentSignal],
    cutoff: float,
) -> dict[str, AgentSignal]:
    """Return *signals* (mutated in place) after considering one hash."""
    workspace_json = hash_dir / "workspace.json"
    chat_sessions = hash_dir / "chatSessions"

    # Rule: both must be present and readable.  Either absence is a skip,
    # not an error — sensors degrade.
    if not workspace_json.is_file():
        return signals
    if not chat_sessions.is_dir():
        return signals

    # Read workspace.json (skip on malformed JSON).
    try:
        text = workspace_json.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return signals

    try:
        payload = json.loads(text)
    except json.JSONDecodeError:
        return signals

    if not isinstance(payload, dict) or "folder" not in payload:
        return signals

    folder_uri = payload["folder"]
    if not isinstance(folder_uri, str):
        return signals

    # Convert file:// URI to an absolute filesystem path.  url2pathname is
    # required over string slicing so percent-encoded escapes (e.g. %20) are
    # decoded instead of left verbatim.
    try:
        raw_cwd = _uri_to_path(folder_uri)
    except (ValueError, TypeError, AttributeError):
        return signals

    # Resolve cold activity: the newest chat file inside chatSessions/.  If
    # the directory is empty (or unreadable) we have no signal time, so skip.
    newest_mtime = _read_chat_session_mtime(chat_sessions, None)
    if newest_mtime is None:
        return signals
    if newest_mtime < cutoff:
        return signals

    at = datetime.fromtimestamp(newest_mtime, tz=timezone.utc)

    # Resolve to the canonical project root; skip the hash if it cannot be
    # resolved so that one broken workspace doesn't break the whole scan.
    try:
        root = resolve_root(raw_cwd, config)
    except Exception:
        return signals

    key = str(root)
    existing = signals.get(key)
    if existing is None or at > existing.at:
        signals[key] = AgentSignal(
            root=key,
            at=at,
            agent="copilot",
            session_id=None,
            event=None,
            raw_cwd=raw_cwd,
        )

    return signals
