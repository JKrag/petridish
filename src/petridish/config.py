"""Runtime configuration for the petridish daemon.

The config is a frozen dataclass whose fields mirror the sections of
``~/.petridish/config.toml`` documented in the implementation plan.  Every
field has a sensible default so a missing (or empty) config file is *valid* —
the daemon should be install-and-run on a fresh machine.

Path-like fields (``roots``, ``extra_paths``) are expanded for ``~`` and
environment variables **at load time**, not at class definition.  That means
users can override the defaults with their own ``~/foo`` paths and still get
the correct expansion.
"""

from __future__ import annotations

import os
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import cast

#: Default project roots the discovery step crawls first.
DEFAULT_ROOTS: tuple[str, ...] = ("~/repos", "~/learning")

#: Default author regexes.  Matched against ``git log --author=``; the first
#: match wins when bucketing.  Kept as raw regex strings because the actual
#: compilation happens in M3, where we can compile+cache per config.
DEFAULT_AUTHOR_PATTERNS: tuple[str, ...] = ("Jan.*Krag",)

#: Directory basenames hard-skipped during crawl.  Exposed as a frozenset
#: because sensors check ``name not in ignore_dirs`` millions of times per tick.
DEFAULT_IGNORE_DIRS: frozenset[str] = frozenset(
    {
        "node_modules",
        ".worktrees",
        "vendor",
        ".venv",
        "venv",
        "target",
        "dist",
        "build",
        ".next",
        "Library",
        ".Trash",
    }
)

#: Thresholds (hours).  A project whose ``last_activity_at`` is younger than
#: ``active`` hours lands in the "active" bucket, etc.  Stored in hours so
#: TOML users can reason about them naturally; consumers convert to seconds.
DEFAULT_BUCKETS: dict[str, float] = {
    "active": 48.0,
    "in_flight": 14 * 24.0,  # 336
    "stale": 60 * 24.0,      # 1440
}


class ConfigError(Exception):
    """Raised when a config file exists but cannot be parsed.

    A *missing* file is not an error (defaults apply); a *malformed* one is,
    because silently falling back to defaults would hide a typo'd config and
    leave the user wondering why their settings did nothing.
    """


@dataclass(frozen=True)
class Config:
    """Radar configuration.

    Every field has a default so ``load_config()`` never needs a real file.
    Instances are immutable: the frozen flag protects callers from accidentally
    mutating a shared ``Config`` held by multiple sensors.

    Path fields are stored as :class:`Path` — expanding ``~`` and env vars
    happens inside :func:`load_config`, never in the class itself.
    """

    roots: tuple[Path, ...] = field(default_factory=lambda: tuple(Path(p) for p in DEFAULT_ROOTS))
    extra_paths: tuple[Path, ...] = field(default_factory=tuple)
    author_patterns: tuple[str, ...] = field(
        default_factory=lambda: tuple(DEFAULT_AUTHOR_PATTERNS)
    )
    author_since: str = "3 years"

    #: Hard-skip dirs by basename during root crawling.
    ignore_dirs: frozenset[str] = field(
        default_factory=lambda: DEFAULT_IGNORE_DIRS
    )

    #: Activity-recency thresholds in hours.  Stored as float for easy comparison
    #: against ``timedelta`` in seconds — consumers in ``petridish.scan`` multiply by
    #: 3600 before handing to the bucketing logic.
    bucket_thresholds: dict[str, float] = field(
        default_factory=lambda: dict(DEFAULT_BUCKETS)
    )

    #: ``{path_glob_or_pattern: category_label}`` overrides.  Empty by default;
    #: the crawl step in M2 consults this map to recategorise a project.
    category_overrides: dict[str, str] = field(default_factory=dict)

    #: Maximum directory depth when crawling roots.  Default is four, which
    #: covers most monorepo layouts (repo → packages/ → core/) without scanning
    #: the entire file system.
    max_depth: int = 4


# ---------------------------------------------------------------------------
# Loading
# ---------------------------------------------------------------------------

_CONFIG_DIR = os.path.join(os.path.expanduser("~"), ".petridish")
_DEFAULT_PATH = os.path.join(_CONFIG_DIR, "config.toml")

#: Literal marker appended to every hook command line the installer (M9)
#: writes into ``~/.claude/settings.json``. Shared by ``cli.py`` (doctor's
#: "hook installed" check) and ``installer.py`` (install/uninstall) so both
#: always agree on what "our entry" means.
HOOK_MARKER = "# petridish"

# Fields that tomllib will parse as ``list[str]`` and we must expand to
# absolute :class:`Path`s on load.  Storing them as tuples of Path matches the
# frozen-dataclass contract: a Config instance stays hashable and immutable.
_PATH_FIELDS: frozenset[str] = frozenset({"roots", "extra_paths"})


def _expand_path(p: str) -> Path:
    """Expand ``~`` and environment variables, return an absolute :class:`Path`."""
    # ``expandvars`` first so a path like ``$HOME/repos`` works; ``expanduser``
    # is forgiving on strings without a leading tilde, so a second pass does no
    # harm and catches ``~`` that may have surfaced after env expansion.
    expanded = os.path.expanduser(os.path.expandvars(p))
    return Path(expanded)


def _warn(message: str) -> None:
    """Report a rejected config value on stderr.

    The daemon's stderr lands in the rotated daemon log, so a bad value stays
    diagnosable.  Deliberately not silent: falling back to a default without
    saying so is how a typo'd threshold becomes an unexplained behaviour change
    six weeks later.
    """
    print(f"petridish: config: {message}", file=sys.stderr)


def _coerce_durations(
    user: object, defaults: dict[str, float]
) -> dict[str, float]:
    """Merge user-supplied bucket thresholds on top of defaults.

    tomllib returns ints for ``48`` and floats for ``48.0``; normalise
    everything to float so later comparison with ``timedelta`` is trivial.
    Missing keys fall back to ``defaults`` rather than raising.

    A user value that isn't a real number (``active_hours = "soon"``, or a
    list) falls back to that key's default too.  It must not raise: this
    runs inside ``load_config``, which the daemon calls before every tick, so
    an exception here doesn't degrade one field — it aborts the whole tick and
    ``projects.json`` silently stops being written (invariant 5).
    """
    user_map: dict[str, object] = (
        cast("dict[str, object]", user) if isinstance(user, dict) else {}
    )
    if user is not None and not isinstance(user, dict):
        _warn(f"bucket_thresholds must be a table, got {type(user).__name__}; using defaults")

    out: dict[str, float] = {}
    for key, default_value in defaults.items():
        if key not in user_map:
            out[key] = float(default_value)
            continue
        raw = user_map[key]
        # bool is an int subclass — `active_hours = true` is a mistake, not 1.0.
        if isinstance(raw, bool) or not isinstance(raw, (int, float)):
            _warn(
                f"bucket_thresholds.{key} must be a number, got "
                f"{type(raw).__name__}; using default {default_value}"
            )
            out[key] = float(default_value)
        else:
            out[key] = float(raw)
    return out


def _coerce_str_list(value: object, default: list[str], key: str) -> list[str]:
    """Return a list of strings, falling back to ``default`` on any bad shape.

    Guards a specific silent-corruption trap: ``list("~/repos")`` does not
    raise, it yields ``['~', '/', 'r', ...]``.  A user who writes
    ``roots = "~/repos"`` (string instead of array) would otherwise get eight
    single-character project roots rather than an error or a sane fallback.
    """
    if not isinstance(value, (list, tuple)):
        _warn(f"{key} must be an array, got {type(value).__name__}; using default")
        return list(default)
    items: list[object] = list(cast("list[object] | tuple[object, ...]", value))
    bad: list[object] = [x for x in items if not isinstance(x, str)]
    if bad:
        _warn(
            f"{key} must contain only strings; ignoring "
            f"{len(bad)} non-string entr{'y' if len(bad) == 1 else 'ies'}"
        )
    return [x for x in items if isinstance(x, str)]


def load_config(path: str | Path | None = None) -> Config:
    """Read a TOML config file and return a frozen :class:`Config`.

    Parameters
    ----------
    path:
        Path to the TOML file.  Defaults to ``~/.petridish/config.toml``.
        If the path does not exist (or points at nothing readable) the
        function returns a :class:`Config` built entirely from defaults —
        *never* raises.

    Raises
    ------
    ConfigError
        If the file exists but is malformed or unreadable.

    Examples
    --------
    >>> load_config("/nonexistent/path.toml").roots  # doctest: +SKIP
    (PosixPath('/Users/you/repos'), PosixPath('/Users/you/learning'))

    >>> # with ~/.petridish/config.toml containing `max_depth = 6`
    >>> load_config().max_depth  # doctest: +SKIP
    6
    """
    if path is None:
        resolved = Path(_DEFAULT_PATH)
    else:
        resolved = Path(os.fspath(path))

    user_overrides: dict[str, object] = {}
    if resolved.is_file():
        try:
            with resolved.open("rb") as fh:
                user_overrides = tomllib.load(fh)
        except tomllib.TOMLDecodeError as exc:
            raise ConfigError(f"malformed config file {resolved}: {exc}") from exc
        except OSError as exc:
            raise ConfigError(f"cannot read config file {resolved}: {exc}") from exc

    # Seed with defaults, then overlay anything the file provided.  This keeps
    # the logic linear — there's no special case for "field present in file".
    cfg: dict[str, object] = {
        "roots": list(DEFAULT_ROOTS),
        "extra_paths": [],
        "author_patterns": list(DEFAULT_AUTHOR_PATTERNS),
        "author_since": "3 years",
        "ignore_dirs": list(DEFAULT_IGNORE_DIRS),
        "bucket_thresholds": dict(DEFAULT_BUCKETS),
        "category_overrides": {},
        "max_depth": 4,
    }

    # Keep the pristine defaults: every coercion below falls back to these
    # per-key rather than raising, so one bad value costs that field and not
    # the whole tick.
    defaults: dict[str, object] = {k: v for k, v in cfg.items()}

    for key, default_value in list(cfg.items()):
        if key in user_overrides:
            uval = user_overrides[key]
            if isinstance(default_value, list):
                cfg[key] = _coerce_str_list(
                    uval, cast("list[str]", default_value), key
                )
            elif isinstance(default_value, dict):
                # Only thresholds and category_overrides land here; treat them
                # uniformly — any non-dict user value is dropped.
                if isinstance(uval, dict):
                    cfg[key] = {
                        str(k): v for k, v in uval.items()  # type: ignore[dict-item]
                    }
                else:
                    cfg[key] = {}  # unexpected type in file; start clean
            else:
                cfg[key] = uval

    # Expand path fields to absolute Path objects *after* user overrides are
    # applied — same code path for defaults and user-provided values.
    # ``_coerce_str_list`` above guarantees these are ``list[str]`` by now.
    for pf in _PATH_FIELDS:
        raw_paths = cast("list[str]", cfg[pf])
        cfg[pf] = tuple(_expand_path(p) for p in raw_paths)

    # Coerce ``ignore_dirs`` back to a frozenset.
    cfg["ignore_dirs"] = frozenset(cast("list[str]", cfg["ignore_dirs"]))

    # Coerce bucket thresholds to float and normalise.
    cfg["bucket_thresholds"] = _coerce_durations(
        cfg.get("bucket_thresholds"), DEFAULT_BUCKETS
    )

    # Category overrides: keys and values must both be strings — this maps a
    # project path to a category name and is handed straight to ``dict.get``
    # against a path key, so a non-string value would surface as a bogus
    # category in every frontend.
    raw_overrides = cfg["category_overrides"]
    if isinstance(raw_overrides, dict):
        cfg["category_overrides"] = {
            str(k): v
            for k, v in cast("dict[object, object]", raw_overrides).items()
            if isinstance(v, str)
        }
    else:
        cfg["category_overrides"] = {}

    # ``max_depth`` must be a non-negative integer.  ``bool`` is an ``int``
    # subclass, so ``max_depth = true`` would otherwise sail through as depth 1.
    depth = cfg["max_depth"]
    if isinstance(depth, bool) or not isinstance(depth, int) or depth < 0:
        if "max_depth" in user_overrides:
            _warn(f"max_depth must be a non-negative integer, got {depth!r}; using 4")
        cfg["max_depth"] = 4

    # ``author_since`` is passed to ``git log --since=``; a non-string would
    # make every git call fail rather than just this one field.
    if not isinstance(cfg["author_since"], str):
        _warn(
            f"author_since must be a string, got "
            f"{type(cfg['author_since']).__name__}; using {defaults['author_since']!r}"
        )
        cfg["author_since"] = defaults["author_since"]

    # ``author_patterns`` from TOML comes as a list; promote to tuple to match
    # the frozen-dataclass contract.
    if isinstance(cfg["author_patterns"], list):
        cfg["author_patterns"] = tuple(cfg["author_patterns"])

    return Config(**cfg)  # type: ignore[arg-type]
