"""Discovery: walk configured roots and surface the user's own repos.

This module contains three tightly coupled primitives that the daemon core
(``petridish.scan``) calls exactly once per tick:

* :func:`resolve_root` — collapse a monorepo subdir onto its enclosing git
  root, stopping at the first ``.git`` found going upward.
* :func:`discover` — crawl :class:`~petridish.config.Config.roots` plus
  ``extra_paths`` and return a de-duplicated list of project paths, applying
  the crawl rules from the implementation plan (§2).
* :func:`is_foreign` — ask git whether the user has authored anything in a
  candidate repo; used to filter out clones they never worked on.

All three are pure with respect to the filesystem apart from their documented
subprocess side-effect in :func:`is_foreign`, which is why the daemon wraps it
in a ``try/except`` at the crawl boundary.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from petridish.config import Config


# Manifest files that qualify a directory as a project even without ``.git``.
# Kept at module level so both ``discover`` and any future sensor can swap the
# list in one place instead of duplicating it.  Ordering matters only for test
# readability.
MANIFEST_FILENAMES = ("package.json", "pyproject.toml", "Cargo.toml", "go.mod")


def resolve_root(cwd: str | Path, config: Config) -> Path:
    """Walk UP from ``cwd`` to the nearest ancestor containing a ``.git`` entry
    and return it.

    This is the reverse of the ``cwd``-collapsing pass in M2: given a working
    directory like ``repo/packages/core``, return the project root ``repo`` so
    a single monorepo session doesn't fragment into three phantom projects.

    The walk checks ``cwd`` itself first, then ascends, stopping at whichever
    comes first:

        1. a directory containing a ``.git`` entry — returned as our answer.
        2. a configured root, the user's home directory, or the filesystem
           root :obj:`/` — the walk is capped and we return ``Path(cwd)``
           unchanged.

    Config roots are a **ceiling, never an answer**.  Returning a root merely
    for being a root would collapse every project sitting directly beneath it
    onto the root itself — with ``roots = ["~/repos"]``, both
    ``~/repos/alpha`` and ``~/repos/beta`` would resolve to ``~/repos`` and
    merge into a single phantom project.  A root *is* returned when it
    genuinely contains ``.git``, which is why the repo check precedes the
    ceiling check.

    Returns :class:`Path(cwd)` unchanged when no ``.git`` is found at or above
    ``cwd`` within those bounds — an uninitialised project directory, or a
    non-repo directory the crawl happened to visit.
    """
    cwd = Path(cwd).resolve()

    # ``/`` has no parent — there is nothing to walk up from.
    if cwd == Path("/"):
        return cwd

    home = Path.home().resolve()
    fs_root = Path("/")
    roots = {r.resolve() for r in config.roots}

    # ``cwd`` itself being a repo is the overwhelmingly common case (a session
    # started at a project root), so check it before ascending at all.
    if (cwd / ".git").exists():
        return cwd

    current = cwd.parent
    while True:
        # Check for the repo BEFORE the ceiling test: a configured root may
        # itself be a git repo, and it should be returned when it genuinely is
        # one — but never merely for being a root.
        if (current / ".git").exists():
            return current

        # Ceiling. Config roots, home and ``/`` cap the walk; none of them is
        # an answer on its own.  Returning a bare root here would collapse
        # every project directly beneath it onto the root itself.
        if current in roots or current == home or current == fs_root:
            break

        parent = current.parent
        if parent == current:
            break  # filesystem root reached without further ascent
        current = parent

    return cwd


def discover(config: Config) -> list[Path]:
    """Return a de-duplicated list of project paths under ``config.roots``
    plus ``config.extra_paths``, crawling each root according to the rules
    in the implementation plan §2.

    Crawl rules (applied in this order, all checked per directory):
        1. Skip dirs whose basename is in ``config.ignore_dirs``.
        2. Skip symlinks (real dir must be a real directory, not a link).
        3. Stop descending as soon as a directory contains ``.git`` — include
           it, do not recurse further.
        4. Also qualify a directory that contains any of the manifest files in
           :data:`MANIFEST_FILENAMES` even without ``.git``.
        5. Descend at most ``config.max_depth`` levels below each root.

    Roots that do not exist are silently ignored — a missing ``~/repos`` on a
    fresh machine must never crash the crawl.
    """
    seen: set[Path] = set()
    results: list[Path] = []

    # Combine roots and extras, dedup by resolved path first so we don't crawl
    # the same directory twice if a user accidentally lists both ``~/repos``
    # and its resolved absolute form.  Sort for deterministic test output.
    roots_to_crawl: list[Path] = sorted(
        {p.resolve() for p in (*config.roots, *config.extra_paths)},
    )

    for root_path in roots_to_crawl:
        _crawl_root(Path(root_path), config, seen, results)

    # Return a stable-sorted list for deterministic output across runs.  Sort
    # by the resolved path — that keeps the crawl order predictable for tests
    # and lets `swab scan` produce stable JSON even when ``os.scandir``'s
    # iteration order differs between runs.
    results.sort()
    return results


def _crawl_root(
    root: Path,
    config: Config,
    seen: set[Path],
    results: list[Path],
) -> None:
    """Walk ``root`` depth-first, populating ``results`` in place.

    Depth is tracked explicitly because ``pathlib`` offers no built-in way to
    limit recursion and ``os.walk`` walks downward while we stop at ``.git``,
    so we need our own walker that can short-circuit per-branch.
    """
    if not root.exists():
        # Missing roots are valid — a fresh user has no ``~/repos`` yet.
        return

    if not root.is_dir():
        # Roots that point at files are silently skipped; they're user error
        # but not worth crashing the crawl for.
        return

    def _should_skip(d: Path) -> bool:
        if d.name in config.ignore_dirs:
            return True
        if d.is_symlink():
            return True
        return False

    def _is_project(dir_path: Path) -> bool:
        if (dir_path / ".git").exists():
            return True
        # Manifest files qualify a dir even without a git repo; this catches
        # uninitialised or bare package dirs that the user is actively editing.
        return any((dir_path / name).is_file() for name in MANIFEST_FILENAMES)

    # Depth-first walk. ``stack`` holds ``(directory, depth_remaining)`` tuples
    # so we can enforce ``max_depth`` without a separate counter object.
    stack = [(root, config.max_depth)]

    while stack:
        dir_path, depth_left = stack.pop()

        if _should_skip(dir_path):
            continue

        # ``_is_project`` is the sole inclusion criterion; everything else is
        # just walk control flow.  We always record a visit to avoid revisiting
        # the same resolved path through different parent directories.
        resolved = dir_path.resolve()
        if resolved not in seen:
            seen.add(resolved)
            if _is_project(dir_path):
                results.append(resolved)
                # Stop descending into a repo — by definition it contains its
                # own .git and any nested repos below are submodules we treat
                # as opaque (see crawl rule §3).  This is what prevents a
                # repo-within-a-repo from appearing twice.
                continue

        # Recurse into children if we have depth budget left.  We don't need
        # to check for ``.git`` here: the next iteration will see the entry
        # and stop descending via the ``_is_project`` branch above.
        if depth_left <= 0:
            continue

        try:
            children = sorted(dir_path.iterdir(), key=lambda p: p.name)
        except PermissionError:
            # A directory we can't read is a failure mode we degrade on — see
            # the "sensors degrade, never abort" invariant in CLAUDE.md.
            continue

        for child in children:
            # Only descend into directories; files and sockets don't become
            # projects, and ``iterdir`` returns them too.  ``.is_dir()`` follows
            # symlinks, so we also check ``not is_symlink`` to honour the
            # symlink-skip rule.
            if child.is_dir() and not child.is_symlink():
                stack.append((child, depth_left - 1))


def is_foreign(path: Path, config: Config) -> bool:
    """Return True if the repo at ``path`` looks like a clone the user never
    worked on.

    Algorithm:
        0. Verify ``path`` actually hosts a git repo — a plain directory is not
           a project, so return ``False`` immediately rather than falling
           through to authorship checks that would otherwise produce nonsense.
        1. Run ``git log -1 --format=%cI --author=<pattern> --since=<horizon>``
           for each pattern in ``config.author_patterns``.  If any command
           returns a non-empty date string, the user authored something —
           return ``False``.
        2. Otherwise, check whether the working tree is dirty via
           ``git status --porcelain``.  A non-empty result is positive evidence
           of involvement — return ``False``.
        3. If neither authorship nor uncommitted work is detected, return
           ``True`` (foreign).

    Any git error — repo does not exist, command not installed, permission
    denied — returns ``False``.  Never raise, never crash the crawl.
    """
    path = Path(path)

    def _run(*args: str) -> str:
        """Run git with the documented safety contract.  Returns stdout or empty."""
        try:
            result = subprocess.run(
                ["git", "-C", str(path), *args],
                check=False,
                capture_output=True,
                text=True,
                timeout=5,
            )
        except (OSError, subprocess.SubprocessError):
            return ""
        if result.returncode != 0:
            return ""
        # Strip trailing whitespace — ``git log`` adds a newline even for the
        # single-commit case, and callers compare against empty.
        return result.stdout.strip()

    # Quick pre-check: make sure this directory actually *is* a git repo before
    # doing anything else.  ``git rev-parse --git-dir`` is the lightest git
    # introspection we can do and doubles as the "any git error → False" case.
    if not _run("rev-parse", "--git-dir"):
        return False

    # Step 1: authorship check.  Short-circuit on the first non-empty result
    # because any matching commit is positive evidence of involvement.
    for pattern in config.author_patterns:
        date_str = _run(
            "log", "-1", "--format=%cI", f"--author={pattern}", f"--since={config.author_since}"
        )
        if date_str:
            return False

    # Step 2: dirty-tree check.  Uncommitted work is stronger evidence than
    # authorship alone — someone might be reviewing a clone without pushing.
    status = _run("status", "--porcelain")
    if status:
        return False

    return True
