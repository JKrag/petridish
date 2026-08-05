"""Runtime configuration for the radar daemon.

The config is a frozen dataclass whose fields mirror the sections of
``~/.project-radar/config.toml`` documented in the implementation plan.  Every
field has a sensible default so a missing (or empty) config file is *valid* —
the daemon should be install-and-run on a fresh machine.

Path-like fields (``roots``, ``extra_paths``) are expanded for ``~`` and
environment variables **at load time**, not at class definition.  That means
users can override the defaults with their own ``~/foo`` paths and still get
the correct expansion.
"""

from __future__ import annotations

import os
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

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
    #: against ``timedelta`` in seconds — consumers in ``radar.scan`` multiply by
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

_CONFIG_DIR = os.path.join(os.path.expanduser("~"), ".project-radar")
_DEFAULT_PATH = os.path.join(_CONFIG_DIR, "config.toml")

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


def _coerce_durations(
    user: dict[str, object] | None, defaults: dict[str, float]
) -> dict[str, float]:
    """Merge user-supplied bucket thresholds on top of defaults.

    tomllib returns ints for ``48`` and floats for ``48.0``; normalise
    everything to float so later comparison with ``timedelta`` is trivial.
    Missing keys fall back to ``defaults`` rather than raising.
    """
    out: dict[str, float] = {}
    for key, default_value in defaults.items():
        if user is None or key not in user:
            out[key] = float(default_value)
        else:
            out[key] = float(user[key])
    return out


def load_config(path: str | Path | None = None) -> Config:
    """Read a TOML config file and return a frozen :class:`Config`.

    Parameters
    ----------
    path:
        Path to the TOML file.  Defaults to ``~/.project-radar/config.toml``.
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

    >>> # with ~/.project-radar/config.toml containing `max_depth = 6`
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

    for key, default_value in cfg.items():
        if key in user_overrides:
            uval = user_overrides[key]
            if isinstance(default_value, list):
                cfg[key] = list(uval)
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
    for pf in _PATH_FIELDS:
        cfg[pf] = tuple(_expand_path(p) for p in cfg[pf])

    # Coerce ``ignore_dirs`` back to a frozenset.
    cfg["ignore_dirs"] = frozenset(cfg["ignore_dirs"])

    # Coerce bucket thresholds to float and normalise.
    cfg["bucket_thresholds"] = _coerce_durations(
        cfg.get("bucket_thresholds"), DEFAULT_BUCKETS
    )

    # Category overrides: empty dict if the file supplied an unexpected type.
    if not isinstance(cfg["category_overrides"], dict):
        cfg["category_overrides"] = {}

    # ``max_depth`` must be a non-negative integer.
    depth = cfg["max_depth"]
    if not isinstance(depth, int) or depth < 0:
        cfg["max_depth"] = 4

    # ``author_patterns`` from TOML comes as a list; promote to tuple to match
    # the frozen-dataclass contract.
    if isinstance(cfg["author_patterns"], list):
        cfg["author_patterns"] = tuple(cfg["author_patterns"])

    return Config(**cfg)  # type: ignore[arg-type]
