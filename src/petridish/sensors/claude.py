"""Claude Code transcript sensor.

Scans the directory laid out like ``~/.claude/projects/`` — one subdirectory
per project, each holding ``<session-id>.jsonl`` transcripts — and emits one
:class:`~petridish.schema.AgentSignal` per transcript. The sensor follows the
"degrade, never abort" rule: an unreadable file or a transcript that yields no
usable fields is silently skipped, and the rest of the scan keeps running.

The contract with every other module in this package:

* **We never derive ``cwd`` from a directory name.** The slug-encoded names of
  Claude's project dirs collapse ``/`` and ``-`` identically, so there is no
  reversible mapping. We always read it from the transcript's own JSON lines.
* **``cwd`` varies inside a session**, so we pick the **last** line that carries
  one. The mtime of the file is what describes "now", not where the session
  started.
* **Trailing truncated JSON** is expected while a session is still being
  appended to; we skip those lines and keep going.

The public entry point is :func:`scan`.
"""

from __future__ import annotations

import json
import os
from datetime import datetime, timezone
from pathlib import Path

from petridish.discovery import resolve_root
from petridish.schema import AgentSignal


TAIL_BYTES = 65_536          # Read ~64KiB from the end of each transcript.
COLD_CUTOFF_DEFAULT_HOURS = 1_440   # 60 days — aggressive history is cheap to skip.


# ---------------------------------------------------------------------------
# Transcript reading
# ---------------------------------------------------------------------------

def _parse_transcript(
    file_path: Path,
    *,
    size: int,
) -> tuple[str | None, str | None]:
    """Scan a transcript file and return ``(session_id, raw_cwd)``.

    Reads from the tail for efficiency: seek to ``max(0, size - TAIL_BYTES)``,
    discard the first partial line if the seek landed mid-line, then iterate
    normally. If no ``cwd`` is found in that window we fall back and scan the
    whole file from the top.

    Returns ``(session_id, raw_cwd)`` where either (or both) may be ``None``.
    Per the "degrade, never abort" rule, a line that fails JSON decoding is
    skipped — truncated trailing JSON is *normal*, not an error.
    """
    # ``session_id`` comes from the *first* line that carries it; ``cwd``
    # comes from the *last* line that carries it. So we walk every parsed line
    # once and keep updating ``cwd`` while freezing ``session_id`` after the
    # first hit.
    session_id: str | None = None
    raw_cwd: str | None = None

    def _scan_lines(lines):
        nonlocal session_id, raw_cwd
        for line in lines:
            line = line.strip()
            if not line:
                continue
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                # Trailing partial JSON — normal in live sessions.  Skip and
                # keep going rather than aborting the whole file.
                continue
            if isinstance(record, dict):
                # ``session_id`` comes from the FIRST line that has one.
                if session_id is None and isinstance(record.get("sessionId"), str):
                    session_id = record["sessionId"]
                # ``cwd`` always takes the LAST line that has one — a session's
                # current directory, not where it started.
                if isinstance(record.get("cwd"), str):
                    raw_cwd = record["cwd"]

    # Fast tail read. If the file fits in one tail-window, this is all we do.
    with file_path.open("rb") as fh:
        seek_pos = max(0, size - TAIL_BYTES)
        fh.seek(seek_pos)
        # ``fh.read()`` gives us bytes starting at ``seek_pos``; decode + split
        # into lines.  If we didn't start at byte 0, the first "line" may be a
        # partial line — drop it before processing so a mid-record seek doesn't
        # leave us holding a dangling half-object.
        if seek_pos > 0:
            try:
                fh.readline()  # drop to end of first partial line
            except OSError:
                pass

        # Now scan every complete line in the tail window.  A truncated trailing
        # JSON object (still being appended to by a live session) will produce
        # a JSONDecodeError on the final chunk; we swallow it per-line above.
        body = fh.read(TAIL_BYTES).decode("utf-8", errors="replace")
        _scan_lines(body.splitlines())

    # Fallback: if we got no cwd at all from the tail window, re-read the whole
    # file.  This is rare (most transcripts have a cwd near the end) but keeps
    # the sensor honest for very small or oddly structured transcripts.
    if raw_cwd is None:
        try:
            text = file_path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            return session_id, None
        _scan_lines(text.splitlines())

    return session_id, raw_cwd


# ---------------------------------------------------------------------------
# Public entry point
# ---------------------------------------------------------------------------

def scan(
    claude_dir: str | Path,
    config,
    *,
    cold_cutoff_hours: float = COLD_CUTOFF_DEFAULT_HOURS,
) -> dict[str, AgentSignal]:
    """Scan ``claude_dir`` and return one :class:`AgentSignal` per project.

    Parameters
    ----------
    claude_dir:
        A directory laid out like ``~/.claude/projects/``: top-level subdirs
        each containing one or more ``<session-id>.jsonl`` transcript files.
    config:
        A :class:`~petridish.config.Config` used by :func:`resolve_root` to collapse
        monorepo subdirs (e.g. ``repo/packages/core`` → ``repo``).
    cold_cutoff_hours:
        Skip files whose mtime is older than this many hours.  Defaults to 60
        days (1440 hours), which keeps a large history cheap to scan.

    Returns
    -------
    dict[str, AgentSignal]
        Keyed by the resolved project root. When two transcripts resolve to the
        same root, only the one with the newest ``at`` timestamp survives.
    """
    claude_dir = Path(claude_dir)
    signals: dict[str, AgentSignal] = {}

    # Seconds since epoch for the cutoff; computed once so a slow file walk
    # doesn't recompute it on every stat().
    now_ts = datetime.now(timezone.utc).timestamp()
    cutoff_ts = now_ts - cold_cutoff_hours * 3_600

    # Two-pass: first discover readable transcripts with fresh enough mtimes,
    # then parse them.  Separating discovery from parsing keeps the two in step:
    # if a file disappears between stat() and open(), we degrade silently rather
    # than crashing.
    transcript_files: list[tuple[Path, float]] = []
    try:
        project_dirs = sorted(claude_dir.iterdir())
    except (OSError, PermissionError):
        # No ~/.claude/projects at all (fresh machine, Claude Code never run),
        # or an unreadable directory. Either way this sensor simply has nothing
        # to report — it must not take the whole scan down with it.
        return signals

    for dir_path in project_dirs:
        if not dir_path.is_dir():
            continue

        # List children with the whole block inside a try/except: a directory we
        # can't list is a failure mode we degrade on (CLAUDE.md invariants §5).
        try:
            entries = sorted(dir_path.iterdir(), key=lambda e: e.name)
        except (OSError, PermissionError):
            continue

        for entry in entries:
            if not entry.is_file():
                continue
            if entry.suffix != ".jsonl":
                continue

            try:
                st = os.stat(entry)
            except (OSError, PermissionError):
                # Unreadable file or vanished mid-scan: skip.
                continue

            if st.st_mtime < cutoff_ts:
                # Cold file — older than ``cold_cutoff_hours``. Skip the open
                # entirely; scanning it would be wasted I/O for no new info.
                continue

            transcript_files.append((entry, st.st_mtime))

    # Now parse each candidate.  Order doesn't matter for correctness because we
    # de-duplicate by ``(root, at)`` with a newest-wins rule in the next step.
    for file_path, mtime in transcript_files:
        try:
            size = os.path.getsize(file_path)
        except OSError:
            # File vanished between stat() and getsize(): degrade silently.
            continue

        file_mtime = datetime.fromtimestamp(mtime, tz=timezone.utc)
        session_id, raw_cwd = _parse_transcript(file_path, size=size)

        if raw_cwd is None:
            # No usable cwd extracted from this transcript — nothing to resolve,
            # nothing to signal.  Common for empty files or ones that only
            # carry metadata lines.
            continue

        try:
            root = resolve_root(raw_cwd, config)
        except Exception:
            # resolve_root should never raise on a plain path + config, but be
            # defensive: a malformed cwd would blow the whole scan otherwise.
            continue

        # If two transcripts resolve to the same root, keep the one with the
        # newest ``at``.  The newer session is the one that reflects current
        # activity, which is what we want to surface.
        existing = signals.get(str(root))
        if existing is not None and existing.at >= file_mtime:
            continue

        signals[str(root)] = AgentSignal(
            root=str(root),
            at=file_mtime,
            agent="claude-code",
            session_id=session_id,
            event=None,
            raw_cwd=raw_cwd,
        )

    return signals


__all__ = ["scan", "COLD_CUTOFF_DEFAULT_HOURS"]
