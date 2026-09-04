//! Project crawl, monorepo-collapse, and authorship filter. Mirrors `src/petridish/discovery.py`.
//!
//! Two invariants live here and must not regress (verbatim from `ARCHITECTURE.md` §0):
//! - F2: "The directory slug is not reversibly decodable ... Never parse the dirname for a
//!   path." `resolve_root` only ever walks the real filesystem, never a Claude-projects slug.
//! - F9: "`cwd` varies within a single transcript ... every raw cwd must be resolved up to
//!   its enclosing project root, or one monorepo session shatters into phantom projects."

use crate::config::Config;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Manifest files that qualify a directory as a project even without `.git`.
/// Mirrors `petridish.discovery.MANIFEST_FILENAMES` — kept at module level so
/// both the project-check and any future crawl helper can share the list.
const MANIFEST_FILENAMES: &[&str] = &["package.json", "pyproject.toml", "Cargo.toml", "go.mod"];

/// Discovered project directories: union of (1) a crawl of `config.roots` (max depth
/// `config.max_depth`, stopping descent at `.git`, hard-skipping `config.ignore_dirs`),
/// (2) `config.extra_paths`, deduped by resolved path. Missing roots are skipped, not errors.
/// Symlinks are not followed.
pub fn discover(config: &Config) -> Vec<PathBuf> {
    // Combine roots and extras, dedup by resolved path first so a user who lists both
    // `~/repos` and its canonical form only gets crawled once. Sort for deterministic
    // test output regardless of `read_dir` ordering.
    let mut seeds: Vec<PathBuf> = config
        .roots
        .iter()
        .chain(config.extra_paths.iter())
        .filter_map(|p| p.canonicalize().ok())
        .collect();
    seeds.sort();
    seeds.dedup();

    let mut seen: HashSet<PathBuf> = HashSet::with_capacity(seeds.len());
    let mut results: Vec<PathBuf> = Vec::new();

    for root in seeds {
        crawl_root(&root, config, &mut seen, &mut results);
    }

    // Stable-sorted output so the rest of the pipeline sees deterministic order.
    results.sort();
    results.dedup();
    results
}

/// Walk `root` depth-first, populating `results` in place. Mirrors Python's `_crawl_root`
/// but uses an explicit `(dir, depth_left)` stack since Rust has no built-in `max_depth`
/// walk primitive. Returns early if a directory is a project (contains `.git` or a manifest)
/// — that stops descent into its children, matching crawl rule §3.
fn crawl_root(
    root: &Path,
    config: &Config,
    seen: &mut HashSet<PathBuf>,
    results: &mut Vec<PathBuf>,
) {
    // Missing roots are valid — a fresh user has no `~/repos` yet, and the daemon must
    // never crash on that. Likewise a root that points at a file, not a directory.
    if !root.exists() || !root.is_dir() {
        return;
    }

    let mut stack = vec![(root.to_path_buf(), config.max_depth)];

    while let Some((dir, depth_left)) = stack.pop() {
        // Hard-skip ignored dirs (do NOT descend into them at all). Symlinks are also skipped
        // — never follow them, per contract.
        if is_ignored(&dir, config) {
            continue;
        }

        // Track by resolved (canonical) path so two roots pointing at the same place
        // don't both get crawled. `canonicalize` failure degrades to "keep going" —
        // a real directory whose canonical form is somehow unreachable is still OK to visit.
        let resolved = match dir.canonicalize() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !seen.insert(resolved) {
            continue; // already visited this resolved path.
        }

        if is_project(&dir) {
            // Record it as a project (resolved path for dedupe consistency).
            results.push(dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()));
            // Stop descending into a repo — by definition it contains its own `.git` and any
            // nested `.git` below would be a submodule we treat as opaque. Mirrors Python:
            // "a repo-within-a-repo from appearing twice."
            continue;
        }

        // Depth budget exhausted — don't descend further.
        if depth_left == 0 {
            continue;
        }

        // Read children, degrading gracefully on permission errors (invariant: "sensors
        // degrade, never abort").
        let children = match dir.read_dir() {
            Ok(entries) => entries.filter_map(|e| e.ok()).collect::<Vec<_>>(),
            Err(_) => continue,
        };

        let mut sorted: Vec<PathBuf> = children.iter().map(|c| c.path()).collect();
        sorted.sort();

        for child in sorted {
            if child.is_dir() && !child.is_symlink() {
                stack.push((child, depth_left - 1));
            }
        }
    }
}

fn is_ignored(dir: &Path, config: &Config) -> bool {
    if dir.is_symlink() {
        return true;
    }
    let name = dir.file_name().and_then(|n| n.to_str());
    match name {
        Some(n) => config.ignore_dirs.contains(n),
        None => false,
    }
}

/// True if `dir` is a project: contains `.git`, OR one of the known manifest files.
fn is_project(dir: &Path) -> bool {
    if dir.join(".git").exists() {
        return true;
    }
    MANIFEST_FILENAMES
        .iter()
        .any(|name| dir.join(name).is_file())
}

/// Resolves `path` as far as the filesystem allows, mirroring Python's default
/// `Path.resolve()` (non-strict): canonicalizes the deepest existing ancestor and
/// re-appends whatever trailing components don't exist, rather than failing outright the
/// way `std::fs::canonicalize` does when the leaf is missing. Falls back to `path` itself
/// unchanged only if not even the filesystem root can be reached (never in practice).
fn resolve_non_strict(path: &Path) -> PathBuf {
    if let Ok(p) = path.canonicalize() {
        return p;
    }
    let mut missing_tail: Vec<&std::ffi::OsStr> = Vec::new();
    let mut ancestor = path;
    loop {
        match ancestor.canonicalize() {
            Ok(resolved) => {
                let mut result = resolved;
                for component in missing_tail.into_iter().rev() {
                    result.push(component);
                }
                return result;
            }
            Err(_) => {
                if let Some(name) = ancestor.file_name() {
                    missing_tail.push(name);
                }
                match ancestor.parent() {
                    Some(parent) => ancestor = parent,
                    None => return path.to_path_buf(),
                }
            }
        }
    }
}

/// Walk up from `cwd` (checking `cwd` itself first, then each ancestor) for the first
/// directory containing `.git`. A configured root, `$HOME`, or `/` caps the ascent, but is
/// only returned if it *itself* contains `.git` (checked before the ceiling test each
/// iteration) — never returned merely for being the ceiling. No `.git` ancestor found within
/// bounds => returns `cwd` unchanged.
///
/// Pre-computes the set of canonicalised configured roots once so every iteration's ceiling
/// check is cheap. Per F2, only the real filesystem is consulted — no slug parsing anywhere.
pub fn resolve_root(cwd: &Path, config: &Config) -> PathBuf {
    // Canonicalise cwd (best-effort, non-strict): resolve against the real FS so a `../`
    // chain or symlinked cwd collapses correctly, matching Python's `Path.resolve()`
    // (non-strict by default — it resolves as far as the filesystem allows and keeps any
    // non-existent trailing components literally, rather than failing outright). This
    // matters here specifically: a signal recorded against a since-deleted directory (e.g.
    // a git worktree that has since been removed with `git worktree remove`) must still
    // walk up to find its still-existing parent repo, not silently skip the whole ascent —
    // confirmed as a real-world regression: a stale worktree signal was returned unresolved
    // (never climbing to the real repo root) because `Path::canonicalize()` in Rust's std
    // errors outright on a non-existent leaf, unlike Python's default `resolve()`.
    let cwd = resolve_non_strict(cwd);

    // `/` has no parent — nothing to walk up from.
    if cwd == Path::new("/") {
        return cwd;
    }

    let home =
        std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/home".to_string()));

    // Ceiling set: configured roots (canonicalised), home, filesystem root. A root is a
    // ceiling, NOT an answer — returned only if it itself holds `.git`.
    let ceilings: HashSet<PathBuf> = config
        .roots
        .iter()
        .filter_map(|r| r.canonicalize().ok())
        .chain(std::iter::once(PathBuf::from("/")))
        .collect();
    let home_resolved = home.canonicalize().unwrap_or(home);

    // Check `cwd` itself first — the overwhelmingly common case (a session started at a
    // project root) hits this branch on the first try.
    if cwd.join(".git").exists() {
        return cwd;
    }

    let mut current = match cwd.parent() {
        Some(p) => p.to_path_buf(),
        None => return cwd,
    };

    loop {
        // Repo check BEFORE ceiling test — this is the key invariant. A configured root
        // that IS a git repo must be returned (it genuinely has `.git`); one that isn't
        // is never returned, no matter how deep we climb. Reversing the order of these
        // checks is what caused the sibling-repo collapse bug shipped on the Python
        // build (see `resolve_root` entry in LEARNINGS.md).
        if current.join(".git").exists() {
            return current;
        }

        // Ceiling check AFTER repo check. A ceiling ends the walk — we return the
        // *original* `cwd` unchanged, never the ceiling itself.
        if ceilings.contains(&current) || current == home_resolved {
            break;
        }

        match current.parent() {
            Some(p) if p != current => current = p.to_path_buf(),
            _ => break, // reached filesystem root with no further ascent possible.
        }
    }

    cwd
}

/// `true` if `path` is a git repo whose most recent commit (within `config.author_since`,
/// via `git log -1 --format=%cI --author=<pattern> --since=<since>`) does not match any of
/// `config.author_patterns` — UNLESS the tree is dirty (`git status --porcelain` non-empty),
/// which is treated as positive evidence of active work and short-circuits to `false`
/// regardless of authorship. A non-repo path is never foreign (`false`). An empty
/// `author_patterns` list means everything is foreign (`true`) for any repo.
///
/// gix-backed — see `git.rs`'s module doc comment for the full backend history. Reuses
/// `git::status_entries`/`git::author_since_revwalk` directly rather than duplicating the
/// gix status/ignore-parity and regex-author-matching logic those functions already encode.
pub fn is_foreign(path: &Path, config: &Config) -> bool {
    // Pre-check: confirm this is actually a git repo before running any authorship logic.
    // A failed `gix::open` degrades to "not a repo" (never foreign) per contract.
    let Ok(repo) = gix::open(path) else {
        return false;
    };

    // Empty author_patterns: no signal at all, so treat every repo as foreign. Mirrors
    // the spec divergence from Python (Python would fall through to the dirty check here).
    if config.author_patterns.is_empty() {
        return true;
    }

    // Dirty-tree check (Rust-specific reversal of Python's order, per spec): any status
    // entry is positive evidence of active work, overriding authorship.
    if !crate::git::status_entries(&repo).is_empty() {
        return false;
    }

    // Authorship check: any pattern matching a commit within the since-horizon means NOT
    // foreign. No pattern matches -> foreign. A query failure on one pattern just means "no
    // match for this one" — continue to the next pattern, never bail out.
    for pattern in &config.author_patterns {
        if crate::git::author_since_revwalk(&repo, pattern, &config.author_since).is_some() {
            return false; // matching commit -> not foreign.
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    // ── shared git fixture helpers (mirror the pattern in `git.rs` tests) ──────────

    const AUTHOR_DATE: &str = "2024-01-15T10:30:00+00:00";
    const COMMITTER_DATE: &str = "2024-01-15T10:30:00+00:00";

    /// Default test author/committer pair — used by tests that don't care which user
    /// authored the commit (dirty/repo-shape tests).
    const TEST_AUTHOR: &[(&str, &str)] = &[
        ("GIT_AUTHOR_DATE", AUTHOR_DATE),
        ("GIT_COMMITTER_DATE", COMMITTER_DATE),
        ("GIT_AUTHOR_NAME", "Test Author"),
        ("GIT_AUTHOR_EMAIL", "author@example.com"),
        ("GIT_COMMITTER_NAME", "Test Committer"),
        ("GIT_COMMITTER_EMAIL", "committer@example.com"),
    ];

    /// Fixed RAII guard for `discover` and `is_foreign` tests that builds on
    /// `std::env::temp_dir()` (no external crate needed) and cleans up via Drop.
    #[derive(Debug)]
    struct Tmp {
        path: PathBuf,
    }

    impl Tmp {
        fn new(suffix: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("swab_discovery_{suffix}_{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("mktemp");
            Self { path }
        }

        /// Initialise a bare git repo with the default test author (no commits yet).
        fn git_init(&self) {
            assert!(
                self.run_git_in(".", &["init", "."]).success(),
                "git init failed in {}",
                self.path.display()
            );
        }

        /// Run `git <args>` inside the fixture, with pinned author dates.
        fn run_git_in(&self, cwd_rel: &str, args: &[&str]) -> std::process::ExitStatus {
            Command::new("git")
                .args(["-C", self.path.join(cwd_rel).to_str().unwrap()])
                .args(args)
                .envs(TEST_AUTHOR.iter().map(|(k, v)| (*k, *v)))
                .spawn()
                .expect("git spawn")
                .wait()
                .expect("git wait")
        }

        /// Commit a single file using the default test author. `contents` goes into
        /// `filename` under the fixture root.
        fn git_commit(&self, filename: &str, contents: &str) {
            fs::write(self.path.join(filename), contents).expect("write file");
            assert!(
                self.run_git_in(".", &["add", filename]).success(),
                "git add failed"
            );
            assert!(
                self.run_git_in(
                    ".",
                    &[
                        "commit",
                        "--no-gpg-sign",
                        "-m",
                        "test commit",
                        "--allow-empty"
                    ]
                )
                .success(),
                "git commit failed"
            );
        }

        /// Commit with a custom author pair — used by the authorship tests.
        fn git_commit_with_author(
            &self,
            filename: &str,
            contents: &str,
            author_env: &[(&str, &str)],
        ) {
            fs::write(self.path.join(filename), contents).expect("write file");
            let tmp_dir = self.path.join(".env_tmp");
            let _ = fs::remove_dir_all(&tmp_dir);
            fs::create_dir_all(&tmp_dir).expect("mktemp env");

            // Commit with pinned author dates + custom author.
            assert!(
                Command::new("git")
                    .args([
                        "-C",
                        self.path.join(filename).parent().unwrap().to_str().unwrap(),
                        "commit",
                        "--no-gpg-sign",
                        "-m",
                        "test commit",
                        "--allow-empty",
                    ])
                    .envs(author_env.iter().map(|(k, v)| (*k, *v)))
                    .envs(
                        TEST_AUTHOR
                            .iter()
                            .map(|(k, v)| (format!("GIT_EXTRA_{k}"), *v))
                    )
                    .spawn()
                    .expect("git spawn")
                    .wait()
                    .expect("git commit wait")
                    .success(),
                "git commit failed in {}",
                self.path.display()
            );
            let _ = fs::remove_dir_all(&tmp_dir);
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    // ── helper for building a `Config` — tests compose what they need, no TOML layer ─

    fn test_config(roots: Vec<PathBuf>, extras: Vec<PathBuf>) -> Config {
        Config {
            roots,
            extra_paths: extras,
            author_patterns: vec!["Jan.*Krag".to_string()],
            author_since: "3 years".to_string(),
            ignore_dirs: ["node_modules", ".venv"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            max_depth: 5,
            ..Config::default()
        }
    }

    // ═══ discover tests ════════════════════════════════════════════════════════════

    // 1. discover finds a plain git repo under a configured root.
    #[test]
    fn discover_finds_plain_git_repo() {
        let tmp = Tmp::new("discover_plain");
        tmp.git_init();

        let results = discover(&test_config(vec![tmp.path.clone()], vec![]));
        // The repo root (canonical) must be in the results. Dedup/sort means we don't
        // care about ordering — just containment.
        assert!(
            results.contains(&tmp.path.canonicalize().unwrap()),
            "expected to find {tmp:?} in discover results: {:?}",
            results
        );
    }

    // 2. discover skips a `node_modules` decoy dir entirely.
    #[test]
    fn discover_skips_node_modules_decoy() {
        let tmp = Tmp::new("discover_nodemodules");
        // Top-level is *not* a repo — otherwise `node_modules` would never be reached.
        // We build node_modules/.git *inside* the crawl area but outside any `.git` so
        // a naïve implementation would still descend into node_modules and record it.
        let nm = tmp.path.join("node_modules");
        fs::create_dir_all(nm.join(".git")).expect("nm mkdir");

        let results = discover(&test_config(vec![tmp.path.clone()], vec![]));
        assert!(
            !results
                .iter()
                .any(|p| p.components().any(|c| c.as_os_str() == "node_modules")),
            "node_modules (or anything inside it) must not appear in results: {:?}",
            results
        );
    }

    // 3. discover does NOT recurse into a repo's own subdirectories once `.git` is found there.
    #[test]
    fn discover_stops_at_top_level_git() {
        let tmp = Tmp::new("discover_stop_at_git");
        // Top-level repo.
        tmp.git_init();
        // Subdirectory that also has `.git` — *should not* be recorded (we stop at the
        // first `.git`).
        let sub = tmp.path.join("sub");
        fs::create_dir_all(sub.join(".git")).expect("sub mkdir");

        let results = discover(&test_config(vec![tmp.path.clone()], vec![]));
        assert_eq!(
            results.len(),
            1,
            "expected only the top-level repo, got {:?}",
            results
        );
        assert_eq!(
            results[0].canonicalize().unwrap(),
            tmp.path.canonicalize().unwrap()
        );
        assert!(
            !results
                .iter()
                .any(|p| p.components().any(|c| c.as_os_str() == "sub")),
            "sub (nested .git below the top-level repo) must not appear"
        );
    }

    // 4. discover includes a manifest-only dir (no `.git`) that's in `extra_paths`.
    #[test]
    fn discover_includes_manifest_only_extra_path() {
        let tmp = Tmp::new("discover_manifest_extra");
        // Standalone manifest dir.
        fs::write(tmp.path.join("package.json"), "{}").expect("write");

        let results = discover(&test_config(vec![], vec![tmp.path.clone()]));
        assert!(
            results.contains(&tmp.path.canonicalize().unwrap()),
            "manifest-only extra_path must be in results: {:?}",
            results
        );
    }

    // 5. discover skips a root that doesn't exist on disk, no error.
    #[test]
    fn discover_skips_missing_root() {
        let results = discover(&test_config(
            vec![PathBuf::from("/tmp/does_not_exist_swab_test_xyzzy99_99")],
            vec![],
        ));
        assert!(
            results.is_empty(),
            "missing root must not crash, got {:?}",
            results
        );
    }

    // 6. discover dedupes two roots that resolve to the same real path.
    #[test]
    fn discover_dedupes_clobbering_roots() {
        let tmp = Tmp::new("discover_dedup");
        // Build two "roots" that are the same dir via a symlink — the symlink-resolver
        // in canonicalize() makes them the same PathBuf. The crawl should run exactly
        // once and emit at most one entry. `target` must actually BE a project (a bare
        // `.git` dir is enough) or discover() correctly finds nothing at all, which is
        // not what this test is checking.
        let target = tmp.path.join("target");
        fs::create_dir_all(&target).expect("mkdir target");
        fs::create_dir_all(target.join(".git")).expect("mkdir target/.git");
        let link = tmp.path.join("link");
        // We can't use `create_dir` to make a symlink directly; use os::unix.
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        #[cfg(not(unix))]
        fs::create_dir_all(&link).expect("symlink dir fallback");

        let results = discover(&test_config(vec![link.clone(), target.clone()], vec![]));
        // The canonical form of the (symlink target) dir should appear at most once.
        let canonical_target = target.canonicalize().unwrap();
        let count = results
            .iter()
            .filter(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()) == canonical_target)
            .count();
        assert_eq!(
            count, 1,
            "expected the canonical target exactly once, got {count}: {:?}",
            results
        );
    }

    // ═══ resolve_root tests ═════════════════════════════════════════════════════════

    // 7. resolve_root on a monorepo subdir resolves up to the `.git` root (the parent).
    #[test]
    fn resolve_root_monorepo_subdir() {
        let tmp = Tmp::new("resolve_mono");
        fs::create_dir_all(tmp.path.join("packages/core")).expect("mkdir monorepo tree");
        fs::create_dir_all(tmp.path.join(".git")).expect("mkdir .git");

        let cfg = test_config(vec![tmp.path.clone()], vec![]);
        let cwd = tmp.path.join("packages/core");
        let result = resolve_root(&cwd, &cfg);

        assert_eq!(
            result.canonicalize().unwrap(),
            tmp.path.canonicalize().unwrap(),
            "monorepo subdir should resolve up to the .git root"
        );
    }

    // Regression: an earlier version of `resolve_root` canonicalized `cwd` up front via
    // `std::fs::canonicalize`, which errors outright on a non-existent leaf -- unlike
    // Python's default `Path.resolve()` (non-strict), which resolves as far as it can and
    // keeps missing trailing components literally. A signal recorded against a
    // since-deleted directory (e.g. a git worktree removed after the transcript that
    // referenced it was written) silently returned unresolved instead of climbing to its
    // still-existing parent repo -- caught only by diffing a real $HOME against Python.
    #[test]
    fn resolve_root_walks_up_from_a_deleted_leaf() {
        let tmp = Tmp::new("resolve_deleted_leaf");
        fs::create_dir_all(tmp.path.join("repo")).expect("mkdir repo");
        fs::create_dir_all(tmp.path.join("repo/.git")).expect("mkdir .git");

        // A leaf that never existed on disk, several levels under the real repo root.
        let deleted_leaf = tmp
            .path
            .join("repo")
            .join(".worktrees")
            .join("removed-branch");

        let cfg = test_config(vec![tmp.path.clone()], vec![]);
        let result = resolve_root(&deleted_leaf, &cfg);

        assert_eq!(
            result,
            tmp.path.join("repo").canonicalize().unwrap(),
            "a non-existent leaf must still walk up to its existing parent repo, not be \
             returned unresolved just because the leaf itself is missing"
        );
    }

    // 8. resolve_root on a path with no `.git` ancestor within bounds returns input unchanged.
    #[test]
    fn resolve_root_no_git_ancestor_returns_input() {
        let tmp = Tmp::new("resolve_nogit");
        // Build a dir tree with NO .git anywhere.
        fs::create_dir_all(tmp.path.join("a/b/c")).expect("mkdir deep");

        let cfg = test_config(vec![tmp.path.clone()], vec![]);
        let cwd = tmp.path.join("a/b/c");
        let result = resolve_root(&cwd, &cfg);

        assert_eq!(
            result.canonicalize().unwrap(),
            cwd.canonicalize().unwrap(),
            "no .git ancestor => must return cwd unchanged"
        );
    }

    // 9. resolve_root never returns $HOME or `/` unless they literally contain `.git`.
    #[test]
    fn resolve_root_never_returns_home_or_root_without_dotgit() {
        let tmp = Tmp::new("resolve_no_home_or_slash");
        // Deep dir tree inside tmp with .git only at the bottom (not at home, not at /).
        fs::create_dir_all(tmp.path.join("a/b/c")).expect("mkdir tree");
        fs::create_dir_all(tmp.path.join(".git")).expect("mkdir .git at bottom");

        let cfg = test_config(vec![], vec![]); // no roots in config -> ceilings are just home + /
        let cwd = tmp.path.join("a/b/c");
        let result = resolve_root(&cwd, &cfg);

        assert_ne!(
            result.canonicalize().unwrap(),
            std::env::home_dir()
                .unwrap_or_default()
                .canonicalize()
                .unwrap_or_default(),
            "must never return home just for being a ceiling"
        );
        assert_ne!(
            result.canonicalize().unwrap(),
            PathBuf::from("/"),
            "must never return / just for being a ceiling"
        );
        // The only .git ancestor is tmp.path itself, so the walk stops there and returns it.
        assert_eq!(
            result.canonicalize().unwrap(),
            tmp.path.canonicalize().unwrap()
        );
    }

    // 10. Two sibling repos both directly under one configured root resolve to themselves,
    //     NOT collapsed into the parent root. Regression test for the bug that shipped on
    //     the Python build (see `~/.claude/skills/delegate-to-local/LEARNINGS.md`).
    #[test]
    fn resolve_root_sibling_repos_resolve_separately() {
        let tmp = Tmp::new("resolve_siblings");
        // Two sibling repos, each with its own .git, directly under the same configured root.
        let alpha = tmp.path.join("alpha");
        let beta = tmp.path.join("beta");
        fs::create_dir_all(alpha.join(".git")).expect("alpha .git");
        fs::create_dir_all(beta.join(".git")).expect("beta .git");

        let cfg = test_config(vec![tmp.path.clone()], vec![]);
        let alpha_result = resolve_root(&alpha, &cfg);
        let beta_result = resolve_root(&beta, &cfg);

        assert_eq!(
            alpha_result.canonicalize().unwrap(),
            alpha.canonicalize().unwrap(),
            "alpha must resolve to itself, not the parent root"
        );
        assert_eq!(
            beta_result.canonicalize().unwrap(),
            beta.canonicalize().unwrap(),
            "beta must resolve to itself, not the parent root"
        );
        assert_ne!(
            alpha_result.canonicalize().unwrap(),
            beta_result.canonicalize().unwrap(),
            "alpha and beta must resolve to different paths (the sibling-repo bug collapses them)"
        );
        assert_ne!(
            alpha_result.canonicalize().unwrap(),
            tmp.path.canonicalize().unwrap(),
            "alpha must NOT resolve to the parent root — that's the bug we're guarding against"
        );
    }

    // ═══ is_foreign tests ══════════════════════════════════════════════════════════

    /// Commit a file into `repo` authored by `author_name`. The commit date is pinned so
    /// authorship filters that look at recency (`--since=...`) see a stable target.
    fn git_commit_as(repo: &Path, author_name: &str) {
        let env: Vec<(&str, &str)> = vec![
            ("GIT_AUTHOR_DATE", AUTHOR_DATE),
            ("GIT_COMMITTER_DATE", COMMITTER_DATE),
            ("GIT_AUTHOR_NAME", author_name),
            ("GIT_AUTHOR_EMAIL", "author@example.com"),
            ("GIT_COMMITTER_NAME", author_name),
            ("GIT_COMMITTER_EMAIL", "committer@example.com"),
        ];
        assert!(
            Command::new("git")
                .args([
                    "-C",
                    repo.to_str().unwrap(),
                    "commit",
                    "--no-gpg-sign",
                    "-m",
                    "test commit",
                    "--allow-empty",
                ])
                .envs(env.iter().map(|(k, v)| (*k, *v)))
                .spawn()
                .expect("git spawn")
                .wait()
                .expect("git wait")
                .success(),
            "git commit failed in {}",
            repo.display()
        );
    }

    // 11. is_foreign on a repo whose last commit's author matches a pattern -> false.
    #[test]
    fn is_foreign_author_match_yields_false() {
        let tmp = Tmp::new("is_foreign_match");
        tmp.git_init();
        git_commit_as(&tmp.path, "Jan Krag");

        let config = test_config(vec![], vec![]);
        // Default `test_config` already has `Jan.*Krag` in author_patterns — assert with that.
        assert!(
            !is_foreign(&tmp.path, &config),
            "matching author must not be foreign"
        );
    }

    // 12. is_foreign on a repo whose last commit's author does NOT match -> true.
    #[test]
    fn is_foreign_no_author_match_yields_true() {
        let tmp = Tmp::new("is_foreign_nomatch");
        tmp.git_init();
        git_commit_as(&tmp.path, "Other Person");

        let config = test_config(vec![], vec![]);
        let cfg = Config {
            author_patterns: vec!["Jan.*Krag".to_string()],
            ..config
        };
        assert!(
            is_foreign(&tmp.path, &cfg),
            "non-matching author must be foreign"
        );
    }

    // 13. is_foreign on a DIRTY repo whose author does NOT match -> false (dirty overrides).
    #[test]
    fn is_foreign_dirty_overrides_author() {
        let tmp = Tmp::new("is_foreign_dirty_override");
        tmp.git_init();
        git_commit_as(&tmp.path, "Other Person");
        // Make the tree dirty with an untracked file.
        fs::write(tmp.path.join("uncommitted.txt"), "data").expect("write dirty file");

        let config = test_config(vec![], vec![]);
        let cfg = Config {
            author_patterns: vec!["Jan.*Krag".to_string()],
            ..config
        };
        assert!(
            !is_foreign(&tmp.path, &cfg),
            "dirty tree must short-circuit to not-foreign even with non-matching author"
        );
    }

    // 14. is_foreign on a non-repo path -> false.
    #[test]
    fn is_foreign_non_repo_yields_false() {
        let tmp = Tmp::new("is_foreign_not_repo");
        // Plain directory, no .git, no manifest.
        let path = tmp.path.join("plain");
        fs::create_dir_all(&path).expect("mkdir");

        let config = test_config(vec![], vec![]);
        assert!(
            !is_foreign(&path, &config),
            "non-repo path must not be foreign"
        );
    }

    // 15. is_foreign with empty `author_patterns` -> true (foreign) for a real repo.
    #[test]
    fn is_foreign_empty_patterns_yields_true() {
        let tmp = Tmp::new("is_foreign_empty_patterns");
        tmp.git_init();
        git_commit_as(&tmp.path, "Someone");

        let config = test_config(vec![], vec![]);
        let cfg = Config {
            author_patterns: vec![], // empty
            ..config
        };
        assert!(
            is_foreign(&tmp.path, &cfg),
            "empty author_patterns means foreign for every repo"
        );
    }
}
