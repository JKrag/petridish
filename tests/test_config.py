"""Tests for ``src/petridish/config.py`` — the M0 verify target."""

from __future__ import annotations

import dataclasses
import os

import pytest

from pathlib import Path

from petridish.config import (
    Config,
    ConfigError,
    DEFAULT_AUTHOR_PATTERNS,
    DEFAULT_BUCKETS,
    DEFAULT_IGNORE_DIRS,
    DEFAULT_ROOTS,
    load_config,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _write_toml(path, text):  # type: (str | os.PathLike, str) -> None
    path = path if isinstance(path, str) else str(path)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


# ---------------------------------------------------------------------------
# Defaults & missing-file behaviour
# ---------------------------------------------------------------------------

def test_load_config_missing_file_returns_defaults(tmp_path):
    """A missing config.toml should give back fully-expanded defaults."""
    cfg = load_config(tmp_path / "does_not_exist.toml")

    # Defaults are templates (``~/repos`` etc.). load_config expands both
    # ``~`` and env vars, so the result is absolute paths — same contract as
    # user-supplied values.
    expected_roots = tuple(
        Path(os.path.expanduser(p)) for p in DEFAULT_ROOTS
    )
    assert cfg.roots == expected_roots
    assert cfg.extra_paths == ()
    assert cfg.author_patterns == DEFAULT_AUTHOR_PATTERNS
    assert cfg.author_since == "3 years"
    assert cfg.max_depth == 4
    # Sanity: expanded paths are absolute.
    assert os.path.isabs(cfg.roots[0])
    assert os.path.isabs(cfg.roots[1])


def test_bucket_threshold_defaults_match_spec(tmp_path):
    """bucket_thresholds must start from the values the spec declares."""
    cfg = load_config(tmp_path / "nope.toml")

    assert cfg.bucket_thresholds == {
        "active": 48.0,
        "in_flight": 14 * 24,  # hours
        "stale": 60 * 24,
    }


# ---------------------------------------------------------------------------
# TOML parsing: full and partial
# ---------------------------------------------------------------------------

def test_full_toml_populates_all_fields(tmp_path):
    toml = tmp_path / "full.toml"
    _write_toml(
        toml,
        """\
roots = ["/a", "/b"]
extra_paths = ["/c"]
author_patterns = ["Alice.*Smith"]
author_since = "5 years"
max_depth = 6
ignore_dirs = ["foo", "bar"]

[category_overrides]
"/weird/path" = "misc"

[bucket_thresholds]
active = 12.5
in_flight = 700
stale = 2000
""",
    )

    cfg = load_config(toml)

    assert cfg.roots == (Path("/a"), Path("/b"))
    assert cfg.extra_paths == (Path("/c"),)
    assert cfg.author_patterns == ("Alice.*Smith",)
    assert cfg.author_since == "5 years"
    assert cfg.max_depth == 6
    # ``ignore_dirs`` is a frozenset (O(1) ``in`` check in sensors, immutability).
    assert isinstance(cfg.ignore_dirs, frozenset)
    assert cfg.ignore_dirs == frozenset({"foo", "bar"})
    assert cfg.category_overrides == {"/weird/path": "misc"}
    # Integer literals from TOML must still coerce to float in our dataclass.
    assert cfg.bucket_thresholds["active"] == 12.5
    assert isinstance(cfg.bucket_thresholds["active"], float)
    assert cfg.bucket_thresholds["in_flight"] == 700.0


def test_partial_toml_missing_fields_fall_back_to_defaults(tmp_path):
    """A TOML that only sets ``roots`` should still give sane other fields."""
    toml = tmp_path / "partial.toml"
    _write_toml(toml, 'roots = ["/x"]\n')

    cfg = load_config(toml)

    assert cfg.roots == (Path("/x"),)
    # All other fields should still carry their defaults — not garbage.
    assert cfg.extra_paths == ()
    assert cfg.author_patterns == DEFAULT_AUTHOR_PATTERNS
    assert cfg.max_depth == 4


def test_toml_with_path_expansion_applied(tmp_path):
    """``~`` and ``$HOME`` must be expanded by load_config, not stored raw."""
    toml = tmp_path / "paths.toml"
    _write_toml(
        toml,
        f"""\
roots = ["~/repos", "${{TMPDIR}}/projects"]
extra_paths = []
""",
    )

    cfg = load_config(toml)

    assert cfg.roots[0] == Path(os.path.expanduser("~/repos"))
    assert cfg.roots[1] == Path(os.path.expandvars("${TMPDIR}/projects"))
    # ``extra_paths`` is empty so the expansion path is a no-op there.
    assert cfg.extra_paths == ()


# ---------------------------------------------------------------------------
# Frozen-dataclass contract
# ---------------------------------------------------------------------------

def test_config_is_frozen(tmp_path):
    """A frozen dataclass must refuse attribute assignment."""
    cfg = load_config(tmp_path / "missing.toml")

    with pytest.raises(dataclasses.FrozenInstanceError):
        cfg.roots = (Path("/something/else"),)  # type: ignore[misc]


def test_ignore_dirs_is_frozenset(tmp_path):
    """Sensor code does ``d in cfg.ignore_dirs`` thousands of times; frozenset
    is both the documented contract and a real performance win over list."""
    cfg = load_config(tmp_path / "missing.toml")

    assert isinstance(cfg.ignore_dirs, frozenset)
    # Sanity: the defaults match what the implementation plan lists.
    assert cfg.ignore_dirs == DEFAULT_IGNORE_DIRS


def test_bucket_thresholds_integers_coerce_to_float():
    """TOML ``48`` parses as int; our dataclass stores float.  Don't let the
    two drift apart silently."""
    cfg = Config()  # defaults, no file touch
    for v in cfg.bucket_thresholds.values():
        assert isinstance(v, float)


# ---------------------------------------------------------------------------
# Malformed input and forward-compatibility
# (added by the orchestrator: the delegated round dropped both of these
# required cases along with the ConfigError they cover)
# ---------------------------------------------------------------------------


def test_malformed_toml_raises_config_error(tmp_path):
    """A file that exists but cannot be parsed must fail loudly.

    Falling back to defaults here would silently ignore a typo'd config and
    leave the user wondering why their settings had no effect.
    """
    bad = tmp_path / "config.toml"
    _write_toml(bad, 'roots = ["~/repos"\nmax_depth = ')  # unterminated

    with pytest.raises(ConfigError):
        load_config(bad)


def test_malformed_toml_error_names_the_file(tmp_path):
    """The raised error should identify which file is broken."""
    bad = tmp_path / "config.toml"
    _write_toml(bad, "this is not = = toml")

    with pytest.raises(ConfigError, match=str(bad.name)):
        load_config(bad)


def test_unknown_key_is_ignored_not_fatal(tmp_path):
    """Unknown keys must not raise — forward-compatibility with newer configs."""
    cfg_file = tmp_path / "config.toml"
    _write_toml(
        cfg_file,
        'max_depth = 7\nsome_future_setting = "value"\n[nested_future]\nx = 1\n',
    )

    cfg = load_config(cfg_file)

    assert cfg.max_depth == 7               # known key still applied
    assert not hasattr(cfg, "some_future_setting")


def test_missing_file_does_not_raise_config_error(tmp_path):
    """Missing != malformed: a nonexistent file is valid and yields defaults."""
    cfg = load_config(tmp_path / "definitely-absent.toml")

    assert cfg.max_depth == 4
    assert cfg.author_since == "3 years"
