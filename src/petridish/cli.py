"""Command-line entry point for ``swab``.

Public API: :func:`main` returns an exit code and never calls ``sys.exit``
itself, so tests can drive it directly.

Subcommands:

- ``swab scan``                     Run a fresh scan and write the state file.
- ``swab list [--bucket|--all|--json]``
                                    Inspect the cached state file (never scans).
- ``swab path <QUERY>``             Print the project path matching QUERY.
- ``swab doctor``                   Check system health and report findings.
- ``swab config``                   Print the config file location, every
                                    field it accepts, and an example.

Config file: ``~/.petridish/config.toml`` (TOML, entirely optional — every
field has a default). See ``swab config`` for the full reference.

Keep stdlib-only: the CLI is part of the "install-and-run" surface and must
work on a brand-new machine with no extra pip packages.
"""

from __future__ import annotations

import argparse
import dataclasses
import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path

from petridish.config import Config, HOOK_MARKER, load_config
from petridish.schema import (
    Radar,
    STATUS_BUCKETS,
    read_json,
    write_atomic,
)

CONFIG_DIR = os.path.join(os.path.expanduser("~"), ".petridish")
_DEFAULT_STATE_PATH = os.path.join(CONFIG_DIR, "projects.json")
_CONFIG_FILE_PATH = os.path.join(CONFIG_DIR, "config.toml")

#: launchd (see M9's plist) redirects the daemon's stdout/stderr here on
#: every 60s tick. launchd itself has no log-rotation facility, so ``scan``
#: truncates it in-process once it crosses this size; the append-mode fd
#: launchd holds keeps writing correctly from offset 0 after truncation.
_DAEMON_LOG_PATH = os.path.join(CONFIG_DIR, "daemon.log")
_DAEMON_LOG_MAX_BYTES = 5 * 1024 * 1024


def _rotate_daemon_log(path: str = _DAEMON_LOG_PATH, max_bytes: int = _DAEMON_LOG_MAX_BYTES) -> None:
    try:
        if os.path.getsize(path) > max_bytes:
            os.truncate(path, 0)
    except OSError:
        pass  # no log file yet (e.g. not installed via launchd) — nothing to rotate


# ---------------------------------------------------------------------------
# Subcommand handlers.  Each returns an exit code (0 = success).
# ---------------------------------------------------------------------------

def _cmd_scan(args: argparse.Namespace) -> int:
    from petridish.config import load_config  # local to keep main module light
    from petridish.scan import write_scan

    _rotate_daemon_log()

    try:
        config = load_config()
        radar = write_scan(config, args.state)
    except Exception as exc:  # sensors degrade — never raise out of main()
        print(f"scan failed: {exc}", file=sys.stderr)
        return 1

    n = len(radar.projects)
    ms = radar.scan_duration_ms
    # ``args.state`` is already expanded by the caller (argparse + resolve); no
    # further tilde-processing needed for the printed path.
    print(f"scanned {n} projects in {ms}ms -> {args.state}")
    return 0


def _cmd_list(args: argparse.Namespace) -> int:
    state_path = args.state
    if not os.path.isfile(state_path):
        print(
            f"no state file at {state_path}; run 'swab scan' first",
            file=sys.stderr,
        )
        return 1

    radar = read_json(state_path)

    projects: list[Radar.projects.__class__.__args__[0]] = list(radar.projects)

    if args.bucket is not None:
        projects = [p for p in projects if p.status_bucket == args.bucket]

    if not args.all:
        projects = [p for p in projects if not p.is_foreign]

    if args.json:
        payload = [p.to_dict() for p in projects]
        print(json.dumps(payload, indent=2))
        return 0

    return _print_table(projects)


def _header_columns() -> list[str]:
    """Column headers, kept in a function so tests can introspect them."""
    return ["bucket", "name", "agent", "branch", "dirty"]


def _print_table(projects: list) -> int:
    """Render a fixed-column table to stdout. Returns 0 on success."""
    cols = _header_columns()

    rows: list[list[str]] = []
    for p in projects:
        # Show WHICH agent even when idle: "claude-code (idle)" beats a bare
        # "idle", because the agent name (and the session behind it) is the
        # thing you act on. Only fall back to the bare state when no agent has
        # ever touched this project.
        agent_label = (
            f"{p.agent.active_agent} ({p.agent.state})"
            if p.agent.active_agent
            else p.agent.state
        )
        dirty = "*" if (p.git.is_repo and p.git.is_dirty) else " "
        rows.append([
            p.status_bucket,
            p.name,
            agent_label,
            p.git.branch or "-",
            dirty,
        ])

    widths = [len(c) for c in cols]
    for row in rows:
        for i, cell in enumerate(row):
            widths[i] = max(widths[i], len(cell))

    header = "  ".join(c.ljust(widths[i]) for i, c in enumerate(cols))
    print(header)
    print("  ".join("-" * w for w in widths))
    for row in rows:
        print("  ".join(cell.ljust(widths[i]) for i, cell in enumerate(row)))

    return 0


def _cmd_path(args: argparse.Namespace) -> int:
    state_path = args.state
    if not os.path.isfile(state_path):
        print(
            f"no state file at {state_path}; run 'swab scan' first",
            file=sys.stderr,
        )
        return 1

    radar = read_json(state_path)
    projects = list(radar.projects)
    query = args.query

    # Priority 1: exact match on name.
    matches = [p for p in projects if p.name == query]
    if len(matches) == 1:
        print(matches[0].path)
        return 0
    # If multiple exact matches (shouldn't happen but be defensive): fall
    # through to substring search.

    # Priority 2: case-insensitive substring of name.
    name_matches = [p for p in projects if query.lower() in p.name.lower()]

    # Priority 3: case-insensitive substring of path.
    path_matches = [p for p in projects if query.lower() in p.path.lower()]

    candidates: list = []
    for p in name_matches:
        if p not in candidates:
            candidates.append((0, p))  # (rank, project)
    for p in path_matches:
        if p not in candidates:
            candidates.append((1, p))

    if not candidates:
        print(f"no project matches {query!r}", file=sys.stderr)
        return 1

    # Best rank first; ties broken by most-recent last_activity_at.
    best = sorted(
        candidates,
        key=lambda x: (x[0], 0.0 if x[1].last_activity_at is None
                       else -x[1].last_activity_at.timestamp()),
    )[0]

    print(best[1].path)
    return 0


def _cmd_doctor(args: argparse.Namespace) -> int:
    """Run system-health checks; report per check and return non-zero if any
    check failed. Never raises — every check has its own try/except envelope
    so a bad filesystem layout can't bring down the CLI.

    Status labels:
    * ``ok:``       check passed, nothing to do.
    * ``WARN:``     something is stale or missing but non-fatal (e.g. a stale
                    state file).
    * ``FAIL:``     a hard break — config won't load, roots are gone, the
                    state file is unreadable, or the hook is unconfigured.
    """
    problems: list[str] = []
    report: dict[str, str] = {}  # check name -> "ok" | "warn" | "fail"

    def _check(name: str, fn) -> None:
        try:
            result = fn()
        except Exception as exc:
            problems.append(f"{name}: {exc}")
            report[name] = "fail"
            return
        if result is True:
            report[name] = "ok"
        else:
            problems.append(f"{name}: {result}")
            report[name] = "fail"

    def _check_config() -> bool:
        try:
            load_config()
        except Exception as exc:
            problems.append(f"config load failed: {exc}")
            return False
        return True

    def _check_roots() -> bool:
        try:
            cfg = load_config()
        except Exception as exc:
            problems.append(f"config load failed: {exc}")
            return False
        missing = [str(p) for p in cfg.roots if not os.path.isdir(p)]
        if missing:
            problems.append(f"roots not found: {', '.join(missing)}")
        return len(missing) == 0

    def _check_state() -> bool:
        state_path = args.state
        if not os.path.isfile(state_path):
            problems.append(f"state file missing: {state_path}")
            return False
        try:
            radar = read_json(state_path)
        except Exception as exc:
            problems.append(f"state file invalid JSON: {exc}")
            return False
        dt = radar.updated_at
        age_h = (datetime.now(timezone.utc) - dt).total_seconds() / 3600.0
        if age_h >= 24:
            problems.append(f"state file stale ({age_h:.1f}h old)")
            report["state"] = "warn"
            return True  # readable, just old
        return True

    def _check_hook() -> bool:
        settings_path = os.path.join(
            os.path.expanduser("~"), ".claude", "settings.json"
        )
        if not os.path.isfile(settings_path):
            problems.append(f"settings.json not found: {settings_path}")
            return False
        try:
            text = Path(settings_path).read_text(encoding="utf-8")
            data = json.loads(text)
        except OSError as exc:
            problems.append(f"cannot read settings.json: {exc}")
            return False
        except json.JSONDecodeError as exc:
            problems.append(f"settings.json not valid JSON: {exc}")
            return False

        # Detect the marker in any hook command value (walks lists and dicts).
        needle = HOOK_MARKER

        def _walk(obj: object) -> bool:
            if isinstance(obj, str):
                return needle in obj
            if isinstance(obj, dict):
                for v in obj.values():
                    if _walk(v):
                        return True
            if isinstance(obj, list):
                for item in obj:
                    if _walk(item):
                        return True
            return False

        if not _walk(data):
            problems.append("swab-hook marker not found in ~/.claude/settings.json")
        return not any(p.startswith("swab-hook") for p in problems)

    _check("config", _check_config)
    _check("roots", _check_roots)
    _check("state", _check_state)
    _check("hook", _check_hook)

    for key, status in report.items():
        print(f"{status}: {key}")

    return 0 if not problems else 1


#: One line of human explanation per :class:`Config` field, keyed by field
#: name so it can't silently drift out of sync if a field is renamed —
#: ``_cmd_config`` iterates ``dataclasses.fields(Config)`` and looks up the
#: description, rather than hand-maintaining a parallel list of names.
_CONFIG_FIELD_HELP: dict[str, str] = {
    "roots": "Directories crawled for projects",
    "extra_paths": "Individual extra project paths, for anything outside roots",
    "author_patterns": 'Regex(es) matched against "git log --author=" to decide "did I write this"',
    "author_since": "How far back git log looks when computing authorship",
    "ignore_dirs": "Directory basenames hard-skipped during crawl",
    "bucket_thresholds": "Hour cutoffs for the active/in_flight/stale/cold status buckets",
    "category_overrides": "{path_glob_or_pattern: category_label} manual recategorisation",
    "max_depth": "How deep the crawl descends into roots before giving up on a subtree",
}


def _format_default(value: object) -> str:
    """Render a Config field's default as it would look written in TOML."""
    if isinstance(value, (tuple, frozenset)):
        items = sorted(str(v) for v in value) if isinstance(value, frozenset) else [str(v) for v in value]
        return "[" + ", ".join(f'"{v}"' for v in items) + "]"
    if isinstance(value, dict):
        if not value:
            return "{}"
        return "{" + ", ".join(f"{k} = {v}" for k, v in value.items()) + "}"
    if isinstance(value, str):
        return f'"{value}"'
    return str(value)


def _cmd_config(args: argparse.Namespace) -> int:
    """Print the config file location, its full field reference, and an
    example. Sourced from :class:`Config`'s own defaults (via
    ``dataclasses.fields``), so it can't drift out of sync with the code."""
    defaults = Config()

    print(f"Config file: {_CONFIG_FILE_PATH}")
    print(
        "Optional TOML file — every field below has a default, so a missing "
        "file, or any field left out, is valid; only what you set overrides "
        "the default.\n"
    )

    for f in dataclasses.fields(Config):
        default = _format_default(getattr(defaults, f.name))
        help_text = _CONFIG_FIELD_HELP.get(f.name, "")
        print(f"  {f.name}")
        print(f"      {help_text}")
        print(f"      default: {default}")

    print()
    print("Example — only override what you care about:\n")
    print('  roots = ["~/repos", "~/work"]')
    print("  max_depth = 6")
    print()
    print("  [bucket_thresholds]")
    print("  active = 24.0")

    return 0


# ---------------------------------------------------------------------------
# Argument parsing and dispatch.
# ---------------------------------------------------------------------------

def _build_parser(state_default: str = _DEFAULT_STATE_PATH) -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="swab",
        description=(
            "petridish CLI: scan, list, path, doctor, config. "
            f"Config file: {_CONFIG_FILE_PATH} (optional, see 'swab config')."
        ),
    )
    parser.add_argument(
        "--state",
        default=state_default,
        help=(
            "Path to the projects.json state file. "
            f"Default: {state_default}"
        ),
    )

    sub = parser.add_subparsers(dest="subcommand", required=True)

    scan_p = sub.add_parser("scan", help="Run a fresh scan and write the state file.")
    scan_p.set_defaults(func=_cmd_scan)

    list_p = sub.add_parser(
        "list",
        help="List projects from the cached state file.",
    )
    list_p.add_argument(
        "--bucket",
        choices=list(STATUS_BUCKETS),
        help="Filter to a single bucket: active|in_flight|stale|cold.",
    )
    list_p.add_argument(
        "--all",
        action="store_true",
        dest="all",
        help="Show projects regardless of is_foreign status.",
    )
    list_p.add_argument(
        "--json",
        action="store_true",
        help="Emit filtered projects as JSON instead of a table.",
    )
    list_p.set_defaults(func=_cmd_list)

    path_p = sub.add_parser(
        "path",
        help="Resolve a query to the matching project's path.",
    )
    path_p.add_argument("query", help="Project name or substring to match.")
    path_p.set_defaults(func=_cmd_path)

    doc = sub.add_parser("doctor", help="Health-check the system.")
    doc.set_defaults(func=_cmd_doctor)

    config_p = sub.add_parser(
        "config",
        help="Print the config file location, its fields, and an example.",
    )
    config_p.set_defaults(func=_cmd_config)

    return parser


def main(argv: list[str] | None = None) -> int:
    """Run the CLI. Returns 0 on success, non-zero on failure.

    Does *not* call ``sys.exit`` — tests drive this directly.
    """
    parser = _build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
