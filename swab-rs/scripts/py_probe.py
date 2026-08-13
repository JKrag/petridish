#!/usr/bin/env python3
"""Per-function ground-truth probe over the real Python implementation.

Used by parity_check.sh as the external oracle for modules R2-R7, where the aggregator
(R8) isn't wired up yet so diff_check.sh's full-scan comparison can't run. Each subcommand
reads one JSON object of arguments from stdin and prints one JSON result to stdout — the
Rust side (swab-rs/examples/probe.rs) implements the identical argv/stdin/stdout contract
so parity_check.sh can run both and diff the output.

Usage: py_probe.py <function> < args.json
"""
from __future__ import annotations

import json
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from petridish.config import Config
from petridish import discovery, git as git_mod
from petridish.sensors import claude as claude_sensor
from petridish.sensors import copilot as copilot_sensor
from petridish.sensors import quota as quota_sensor
from petridish import events as events_mod


def _config(args: dict) -> Config:
    return Config(
        roots=tuple(Path(p) for p in args.get("roots", [])),
        extra_paths=tuple(Path(p) for p in args.get("extra_paths", [])),
        author_patterns=tuple(args.get("author_patterns", ())),
        author_since=args.get("author_since", "3 years"),
        ignore_dirs=frozenset(args.get("ignore_dirs", ())) or frozenset(
            {"node_modules", ".worktrees", "vendor", ".venv", "venv", "target", "dist", "build", ".next", "Library", ".Trash"}
        ),
        max_depth=args.get("max_depth", 4),
    )


def _iso(dt: datetime | None) -> str | None:
    if dt is None:
        return None
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")


def _signal_to_dict(sig) -> dict:
    return {
        "root": sig.root,
        "at": _iso(sig.at),
        "agent": sig.agent,
        "session_id": sig.session_id,
        "event": sig.event,
        "raw_cwd": sig.raw_cwd,
    }


def _signals_map_to_dict(signals: dict) -> dict:
    return {root: _signal_to_dict(sig) for root, sig in signals.items()}


def cmd_resolve_root(args: dict) -> dict:
    root = discovery.resolve_root(args["cwd"], _config(args))
    return {"root": str(root)}


def cmd_is_foreign(args: dict) -> dict:
    return {"foreign": discovery.is_foreign(Path(args["path"]), _config(args))}


def cmd_git_scan(args: dict) -> dict:
    gs = git_mod.scan(
        args["path"],
        tuple(args.get("author_patterns", ())),
        args.get("author_since", "3 years"),
    )
    return {
        "is_repo": gs.is_repo,
        "branch": gs.branch,
        "is_dirty": gs.is_dirty,
        "uncommitted_files": gs.uncommitted_files,
        "last_commit_at": _iso(gs.last_commit_at),
        "mine_last_commit_at": _iso(gs.mine_last_commit_at),
        "github_url": gs.github_url,
    }


def cmd_claude_scan(args: dict) -> dict:
    signals = claude_sensor.scan(
        args["claude_dir"],
        _config(args),
        cold_cutoff_hours=args.get("cold_cutoff_hours", 1440),
    )
    return _signals_map_to_dict(signals)


def cmd_copilot_scan(args: dict) -> dict:
    signals = copilot_sensor.scan(
        args["workspace_storage_dir"],
        _config(args),
        cold_cutoff_hours=args.get("cold_cutoff_hours", 1440),
    )
    return _signals_map_to_dict(signals)


def cmd_events_read_and_compact(args: dict) -> dict:
    signals = events_mod.read_and_compact(
        args["path"],
        _config(args),
        max_bytes=args.get("max_bytes", 5_242_880),
    )
    return _signals_map_to_dict(signals)


def cmd_quota_read(args: dict) -> dict:
    now = datetime.fromisoformat(args["now"]) if "now" in args else None
    qs = quota_sensor.read_quota(args.get("home"), now=now)
    if qs is None:
        return {"quota": None}
    return {
        "quota": {
            "measured_at": _iso(qs.measured_at),
            "five_hour_used_pct": qs.five_hour_used_pct,
            "five_hour_resets_at": _iso(qs.five_hour_resets_at),
            "seven_day_used_pct": qs.seven_day_used_pct,
            "seven_day_resets_at": _iso(qs.seven_day_resets_at),
            "context_used_pct": qs.context_used_pct,
        }
    }


DISPATCH = {
    "resolve_root": cmd_resolve_root,
    "is_foreign": cmd_is_foreign,
    "git_scan": cmd_git_scan,
    "claude_scan": cmd_claude_scan,
    "copilot_scan": cmd_copilot_scan,
    "events_read_and_compact": cmd_events_read_and_compact,
    "quota_read": cmd_quota_read,
}


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] not in DISPATCH:
        print(f"usage: py_probe.py <{'|'.join(DISPATCH)}> < args.json", file=sys.stderr)
        return 2
    args = json.load(sys.stdin)
    result = DISPATCH[sys.argv[1]](args)
    json.dump(result, sys.stdout, sort_keys=True)
    print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
