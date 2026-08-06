"""Tests for ``src/petridish/discovery.py``.

Real fixture trees are constructed with ``tmp_path`` and ``git init``, not
mocks — this module has to survive on a machine that has a working ``git``
binary but no internet, so any test that depends on ``subprocess.run`` to do
our job for us would hide exactly the bugs it is supposed to surface.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

from petridish.config import Config
from petridish.discovery import discover, is_foreign, resolve_root


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _init_repo(dir_path: Path, message: str = "initial commit") -> None:
    """Initialise a git repo at ``dir_path`` and make one commit.

    Pinned author/date so tests don't depend on the test runner's identity or
    timezone — otherwise every commit message would differ between machines.
    Author/date are written into the repo's local config so the parent
    process's ``user.signingkey`` cannot sneak in and force GPG on us.
    """
    subprocess.run(
        ["git", "init", "-q", "--initial-branch=main", str(dir_path)],
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "-C", str(dir_path), "config", "commit.gpgsign", "false"],
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "-C", str(dir_path), "config", "user.name", "Test"],
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "-C", str(dir_path), "config", "user.email", "t@e.st"],
        check=True,
        capture_output=True,
    )
    # Touch a file so there is something staged to commit.
    (dir_path / "README.md").write_text("# test\n")
    subprocess.run(
        ["git", "-C", str(dir_path), "add", "."],
        check=True,
        capture_output=True,
    )
    env = os.environ.copy()
    env["GIT_AUTHOR_DATE"] = "2024-01-01T00:00:00Z"
    env["GIT_COMMITTER_DATE"] = "2024-01-01T00:00:00Z"
    result = subprocess.run(
        ["git", "-C", str(dir_path), "commit", "-q", "--no-gpg-sign", "-m", message],
        check=False,
        capture_output=True,
        env=env,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"git commit failed in {dir_path}: rc={result.returncode} "
            f"stderr={result.stderr.decode(errors='replace')}"
        )


def _make_config(
    roots: tuple[Path, ...] = (),
    extra_paths: tuple[Path, ...] = (),
    author_patterns: tuple[str, ...] | None = None,
    ignore_dirs: frozenset[str] | None = None,
    max_depth: int = 4,
) -> Config:
    """Construct a Config with sensible defaults and only the caller-chosen
    overrides.  Keeps test bodies short and lets authors focus on the
    behaviour being tested rather than Config plumbing."""
    return Config(
        roots=tuple(Path(r) for r in roots),
        extra_paths=tuple(Path(p) for p in extra_paths),
        author_patterns=author_patterns if author_patterns is not None else ("Test",),
        ignore_dirs=ignore_dirs if ignore_dirs is not None else frozenset(
            {"node_modules", ".worktrees", "vendor", ".venv", "venv",
             "target", "dist", "build", ".next", "Library", ".Trash"}
        ),
        max_depth=max_depth,
    )


# ---------------------------------------------------------------------------
# discover: behaviour tests
# ---------------------------------------------------------------------------

def test_discover_finds_plain_git_repo_under_root(tmp_path):
    """discover must surface a repo sitting directly under a root."""
    repo = tmp_path / "my-repo"
    repo.mkdir()
    _init_repo(repo)

    cfg = _make_config(roots=(tmp_path,))
    found = discover(cfg)

    # The discovered path is the resolved absolute; tmp_path itself is
    # not a repo and should not appear.
    assert repo.resolve() in found
    assert tmp_path.resolve() not in found


def test_discover_skips_node_modules_containing_repo(tmp_path):
    """node_modules is in the hard-skip list and must not be crawled even when
    it contains a .git directory."""
    parent = tmp_path / "app"
    parent.mkdir()
    # Put an unrelated file so ``discover`` visits the dir at all.
    (parent / "index.js").write_text("")
    nm = parent / "node_modules" / "some-pkg"
    nm.mkdir(parents=True)
    _init_repo(nm)

    cfg = _make_config(roots=(tmp_path,))
    found = discover(cfg)

    assert nm.resolve() not in found


def test_discover_does_not_recurse_into_nested_repo(tmp_path):
    """Once a .git directory is encountered, the crawl must stop descending —
    a submodule inside an existing repo must not show up as its own project."""
    outer = tmp_path / "outer-repo"
    outer.mkdir()
    _init_repo(outer)

    # Now drop a second repo inside it.  This simulates what ``git clone`` of
    # a submodule looks like on disk, but from the crawler's perspective it is
    # identical to an arbitrary nested .git directory.
    inner = outer / "packages" / "core"
    inner.mkdir(parents=True)
    _init_repo(inner)

    cfg = _make_config(roots=(tmp_path,))
    found = discover(cfg)

    assert outer.resolve() in found
    # The inner repo MUST NOT be discovered: the crawl stops at ``outer/.git``.
    assert inner.resolve() not in found


def test_discover_includes_manifest_without_git(tmp_path):
    """A directory with pyproject.toml but no .git is still a project."""
    proj = tmp_path / "py-proj"
    proj.mkdir()
    (proj / "pyproject.toml").write_text("[project]\nname = 'x'\n")

    cfg = _make_config(roots=(tmp_path,))
    found = discover(cfg)

    assert proj.resolve() in found


def test_discover_ignores_nonexistent_root_without_raising(tmp_path):
    """A root that does not exist must simply produce no projects — the crawl
    should be resilient to stale configuration."""
    cfg = _make_config(roots=(tmp_path / "does_not_exist",))
    found = discover(cfg)

    assert found == []


def test_discover_depth_limit_halts_crawl(tmp_path):
    """max_depth must bound recursion even when repos keep nesting."""
    # Build repo -> subrepo -> subsubrepo with max_depth=1: only the top should
    # be discovered.
    top = tmp_path / "top"
    top.mkdir()
    _init_repo(top)
    sub = top / "sub"
    sub.mkdir()
    _init_repo(sub)
    deep = sub / "deep"
    deep.mkdir()
    _init_repo(deep)

    # depth=1 means: enter the root (depth 0), recurse into children with
    # depth_left=0, which the walker treats as "do not descend further".
    cfg = _make_config(roots=(tmp_path,), max_depth=1)
    found = discover(cfg)

    assert top.resolve() in found
    # With depth=1 we can descend one level into ``sub`` (it appears as a
    # child at depth_left=1), but not into ``deep``.  Whether ``sub`` itself
    # appears depends on its own depth budget; with max_depth=1 it does not.
    assert sub.resolve() not in found
    assert deep.resolve() not in found


def test_discover_de_duplicates_by_resolved_path(tmp_path):
    """Listing the same root twice (absolute + relative) must yield one entry."""
    repo = tmp_path / "dupe"
    repo.mkdir()
    _init_repo(repo)

    cfg = _make_config(roots=(tmp_path, tmp_path))
    found = discover(cfg)

    # Exactly one entry for this repo — not two, not zero.
    assert sum(1 for p in found if p == repo.resolve()) == 1


def test_discover_skips_symlinks(tmp_path):
    """A symlink to a repo must not be crawled — only real directories count.

    We do still discover the real repo (it is the project); we just don't
    get a second entry for it via the symlink.  Verifying ``sum(...) == 1``
    is what actually proves "symlinks are not crawled" — asserting that the
    symlink's resolved path is absent would falsely fail because we DO crawl
    the real directory underneath it.
    """
    real = tmp_path / "real-repo"
    real.mkdir()
    _init_repo(real)

    link = tmp_path / "link-to-repo"
    link.symlink_to(real)

    cfg = _make_config(roots=(tmp_path,))
    found = discover(cfg)

    # Real repo should appear exactly once — not twice via link → real.
    assert sum(1 for p in found if p == real.resolve()) == 1

    # Confirm the link target is not a duplicate entry.
    assert found.count(real.resolve()) == 1


# ---------------------------------------------------------------------------
# resolve_root: behaviour tests
# ---------------------------------------------------------------------------

def test_resolve_root_maps_monorepo_subdir_to_repo(tmp_path):
    """Walking up from repo/packages/core must land on repo, not on the cwd."""
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo)
    core = repo / "packages" / "core"
    core.mkdir(parents=True)

    cfg = _make_config(roots=(tmp_path,))
    assert resolve_root(core, cfg) == repo.resolve()


def test_resolve_root_unchanged_without_git_ancestor(tmp_path):
    """No ``.git`` anywhere up the tree means the cwd itself is returned."""
    leaf = tmp_path / "a" / "b" / "c"
    leaf.mkdir(parents=True)

    cfg = _make_config(roots=(tmp_path,))
    assert resolve_root(leaf, cfg) == leaf.resolve()


def test_resolve_root_of_repo_under_root_is_the_repo(tmp_path):
    """A configured root caps the walk but is never itself the answer.

    Corrected by the orchestrator: as delegated, this test's docstring claimed
    "a config root sits between the cwd and the .git", but the fixture puts the
    .git in ``inner`` itself — there is no .git above the root at all. It then
    asserted that a genuine repo resolves to its PARENT, which is the
    project-collapsing bug rather than the ceiling behaviour it meant to pin.
    The scenario the old docstring described is covered by
    ``test_resolve_root_does_not_escape_above_configured_root`` below.
    """
    root = tmp_path / "root-dir"
    root.mkdir()
    inner = root / "repo"
    inner.mkdir()
    _init_repo(inner)

    cfg = _make_config(roots=(root,))
    assert resolve_root(inner, cfg) == inner.resolve()


def test_resolve_root_never_returns_home_or_slash(tmp_path):
    """Even on pathological inputs, we must not leak home dir or filesystem root."""
    cfg = _make_config()

    # Absolute root: the walker's guard should short-circuit before we try
    # to go above it, and the explicit sentinel check must return cwd (/).
    assert resolve_root(Path("/"), cfg) == Path("/")


# ---------------------------------------------------------------------------
# is_foreign: behaviour tests
# ---------------------------------------------------------------------------

def _make_clean_repo(dir_path: Path, author_name: str) -> None:
    """Initialise a repo and commit once on behalf of ``author_name``."""
    _init_repo(dir_path)
    # Override the author on the existing commit so we can test foreign vs. own.
    env = os.environ.copy()
    env.update({
        "GIT_AUTHOR_NAME": author_name,
        "GIT_AUTHOR_EMAIL": f"{author_name.lower().replace(' ', '.')}@example.com",
        "GIT_COMMITTER_NAME": author_name,
        "GIT_COMMITTER_EMAIL": f"{author_name.lower().replace(' ', '.')}@example.com",
    })
    # Rewrite the previous commit's author in place.  ``--no-verify`` avoids
    # hook noise; ``--allow-empty`` is unnecessary because we already have one.
    subprocess.run(
        ["git", "-C", str(dir_path), "commit", "--amend", "-q",
         "--no-gpg-sign", "--allow-empty", "--reset-author", "-m", "initial"],
        check=True,
        capture_output=True,
        env=env,
    )


def test_is_foreign_true_for_other_author(tmp_path):
    """A repo whose only commit is by someone else must be flagged foreign."""
    repo = tmp_path / "their-repo"
    repo.mkdir()

    # _make_clean_repo already calls _init_repo; calling it here too left the
    # second commit with nothing staged (rc=1, "nothing to commit").
    _make_clean_repo(repo, "Other Person")

    cfg = _make_config(author_patterns=("Test",))
    assert is_foreign(repo, cfg) is True


def test_is_foreign_false_when_user_authored(tmp_path):
    """A commit matching any author_pattern short-circuits the check."""
    repo = tmp_path / "my-repo"
    repo.mkdir()
    _init_repo(repo)  # already authored by "Test"

    cfg = _make_config(author_patterns=("Test",))
    assert is_foreign(repo, cfg) is False


def test_is_foreign_false_for_dirty_tree(tmp_path):
    """Uncommitted work is positive evidence of involvement even when the user
    never authored a commit."""
    repo = tmp_path / "clean-repo"
    repo.mkdir()
    _make_clean_repo(repo, "Someone Else")

    # Create an uncommitted change — the working tree is dirty now.
    (repo / "README.md").write_text("# modified\n")

    cfg = _make_config(author_patterns=("Test",))
    assert is_foreign(repo, cfg) is False


def test_is_foreign_handles_non_repo_gracefully(tmp_path):
    """A plain directory (no .git) must return False, never raise."""
    plain = tmp_path / "plain"
    plain.mkdir()

    cfg = _make_config()
    assert is_foreign(plain, cfg) is False


def test_is_foreign_empty_author_patterns_returns_true(tmp_path):
    """No author patterns means no commit can ever match — every repo is foreign."""
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo)  # "Test" is the author, but patterns list is empty

    cfg = _make_config(author_patterns=())
    assert is_foreign(repo, cfg) is True


def test_is_foreign_uses_author_since_horizon(tmp_path):
    """Only commits since author_since count — an old commit does not count."""
    repo = tmp_path / "old-repo"
    repo.mkdir()
    _init_repo(repo)

    # Rewrite author to "Test" but pretend the commit is older than 3 years.
    env = os.environ.copy()
    env.update({"GIT_AUTHOR_NAME": "Test", "GIT_AUTHOR_EMAIL": "t@e.st"})
    env["GIT_AUTHOR_DATE"] = "2020-01-01T00:00:00Z"
    env["GIT_COMMITTER_DATE"] = "2020-01-01T00:00:00Z"
    subprocess.run(
        ["git", "-C", str(repo), "commit", "--amend", "-q",
         "--no-gpg-sign", "--allow-empty", "--reset-author", "-m", "old"],
        check=True,
        capture_output=True,
        env=env,
    )

    # Default ``author_since`` is "3 years"; a commit from 2020 is older than
    # that (in any test window), so the repo should be foreign.
    cfg = _make_config(author_patterns=("Test",))
    assert is_foreign(repo, cfg) is True


# ---------------------------------------------------------------------------
# resolve_root regression tests (added by the orchestrator)
#
# The delegated suite passed 18/18 while resolve_root returned the CONFIGURED
# ROOT for any repo sitting directly beneath one — collapsing every such
# project onto the root. These pin the boundary behaviour.
# ---------------------------------------------------------------------------


def test_resolve_root_repo_directly_under_configured_root(tmp_path):
    """A repo one hop below a root must resolve to itself, not to the root."""
    root = tmp_path / "repos"
    root.mkdir()
    repo = root / "myrepo"
    repo.mkdir()
    (repo / ".git").mkdir()

    cfg = _make_config(roots=(root,))

    assert resolve_root(repo, cfg) == repo.resolve()


def test_resolve_root_two_sibling_repos_do_not_collapse(tmp_path):
    """The failure this guards: distinct projects merging into one entry."""
    root = tmp_path / "repos"
    root.mkdir()
    alpha = root / "alpha"
    alpha.mkdir()
    (alpha / ".git").mkdir()
    beta = root / "beta"
    beta.mkdir()
    (beta / ".git").mkdir()

    cfg = _make_config(roots=(root,))

    assert resolve_root(alpha, cfg) != resolve_root(beta, cfg)


def test_resolve_root_monorepo_subdir_under_root(tmp_path):
    """Subdir collapse must still work when the repo sits under a root."""
    root = tmp_path / "repos"
    root.mkdir()
    repo = root / "mono"
    repo.mkdir()
    (repo / ".git").mkdir()
    pkg = repo / "packages" / "core"
    pkg.mkdir(parents=True)

    cfg = _make_config(roots=(root,))

    assert resolve_root(pkg, cfg) == repo.resolve()


def test_resolve_root_returns_root_when_root_is_itself_a_repo(tmp_path):
    """A root that genuinely is a repo is a legitimate answer."""
    root = tmp_path / "repos"
    root.mkdir()
    (root / ".git").mkdir()
    plain = root / "subdir"
    plain.mkdir()

    cfg = _make_config(roots=(root,))

    assert resolve_root(plain, cfg) == root.resolve()


def test_resolve_root_does_not_escape_above_configured_root(tmp_path):
    """A .git above the root must not be reachable through the ceiling."""
    outer = tmp_path / "outer"
    outer.mkdir()
    (outer / ".git").mkdir()
    root = outer / "repos"
    root.mkdir()
    plain = root / "notarepo"
    plain.mkdir()

    cfg = _make_config(roots=(root,))

    assert resolve_root(plain, cfg) == plain.resolve()
