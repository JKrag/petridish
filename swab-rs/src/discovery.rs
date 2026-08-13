//! Project crawl, monorepo-collapse, and authorship filter. Mirrors `src/petridish/discovery.py`.
//!
//! Two invariants live here and must not regress (verbatim from `IMPLEMENTATION_PLAN.md` §0):
//! - F2: "The directory slug is not reversibly decodable ... Never parse the dirname for a
//!   path." `resolve_root` only ever walks the real filesystem, never a Claude-projects slug.
//! - F9: "`cwd` varies within a single transcript ... every raw cwd must be resolved up to
//!   its enclosing project root, or one monorepo session shatters into phantom projects."

use crate::config::Config;
use std::path::{Path, PathBuf};

/// Discovered project directories: union of (1) a crawl of `config.roots` (max depth
/// `config.max_depth`, stopping descent at `.git`, hard-skipping `config.ignore_dirs`),
/// (2) `config.extra_paths`, deduped by resolved path. Missing roots are skipped, not errors.
/// Symlinks are not followed.
pub fn discover(_config: &Config) -> Vec<PathBuf> {
    todo!("R4: walk roots to max_depth, stop at .git, skip ignore_dirs, dedupe by resolved path")
}

/// Walk up from `cwd` (checking `cwd` itself first, then each ancestor) for the first
/// directory containing `.git`. A configured root, `$HOME`, or `/` caps the ascent, but is
/// only returned if it *itself* contains `.git` (checked before the ceiling test each
/// iteration) — never returned merely for being the ceiling. No `.git` ancestor found within
/// bounds => returns `cwd` unchanged.
pub fn resolve_root(cwd: &Path, _config: &Config) -> PathBuf {
    todo!("R4: ascend from cwd checking for .git each step, respecting root/home/'/' as a non-returned ceiling; cwd={cwd:?}")
}

/// `true` if `path` is a git repo whose most recent commit (within `config.author_since`,
/// via `git log -1 --format=%cI --author=<pattern> --since=<since>`) does not match any of
/// `config.author_patterns` — UNLESS the tree is dirty (`git status --porcelain` non-empty),
/// which is treated as positive evidence of active work and short-circuits to `false`
/// regardless of authorship. A non-repo path is never foreign (`false`). An empty
/// `author_patterns` list means everything is foreign (`true`) for any repo.
///
/// Use `crate::git::run_git` (implemented in R3, before this module) for both the dirty-tree
/// check and the author-match check — do not write a second git-timeout wrapper here.
pub fn is_foreign(_path: &Path, _config: &Config) -> bool {
    todo!("R4: crate::git::run_git for dirty-tree + author-match, per config.author_patterns")
}
