//! Git facts for one path. Mirrors `src/petridish/git.py`.
//!
//! ## EXPERIMENTAL: git2 (libgit2) backend, not the CLI-subprocess backend
//!
//! This is a throwaway benchmark branch (`experiment/git2-backend`) swapping the original
//! `git` CLI subprocess implementation for `git2` (Rust bindings to libgit2, the same
//! library Cargo itself uses). The motivation: the subprocess version's wall-clock time was
//! dominated by OS `sys` time (process fork/exec overhead, ~7-9s out of ~28s for an 80-repo
//! scan) rather than actual git work (`user` time, ~2.7s) — `git2` does the same work
//! in-process against an already-open repo handle, with no process spawn at all. See the
//! `experiment/git2-backend` branch's own README/commit message for the measured result.
//!
//! Invariant #6 (`CLAUDE.md`) still applies in spirit ("a git failure is a
//! `GitState(is_repo=False)`, never an exception") but its literal mechanism (5s subprocess
//! timeout via `wait-timeout`) no longer applies: there is no child process to hang. Every
//! libgit2 call here is synchronous and in-process; a failure is a `Result::Err` handled
//! inline, never a panic.
//!
//! Known behavioral gaps versus the original CLI-subprocess implementation (acceptable for
//! a benchmark prototype, would need closing before this could replace the CLI version):
//! - `--since=<horizon>` parsing: the CLI version hands the raw string straight to git's own
//!   approxidate parser (accepts "3 years", "yesterday", ISO dates, etc). This version only
//!   understands `<N> years|months|weeks|days` (optionally pluralized) and falls back to "no
//!   cutoff" (include everything) on anything else — sufficient for this codebase's only
//!   real usage (`Config::default().author_since == "3 years"`) but not a general parser.
//! - `--author=<pattern>` matching: the CLI version delegates to git's own regex engine
//!   against the exact `Name <email>` header line. This version builds the same
//!   `"{name} <{email}>"` string and matches it with the `regex` crate, case-insensitively
//!   (`(?i)` prefix) to approximate git's default author-match case-insensitivity. Patterns
//!   that don't compile as a Rust regex are skipped (degrade), not a hard error.

use crate::schema::GitState;
use chrono::{DateTime, TimeZone, Utc};
use std::path::Path;

/// Very small `--since=<N> <unit>` parser covering this codebase's actual usage (the
/// default and only configured value is `"3 years"`). Returns `None` (no cutoff — include
/// everything) on anything it doesn't recognize, rather than guessing.
pub(crate) fn parse_since_horizon(since: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let parts: Vec<&str> = since.trim().split_whitespace().collect();
    let [n_str, unit] = parts.as_slice() else {
        return None;
    };
    let n: i64 = n_str.parse().ok()?;
    let unit = unit.trim_end_matches('s').to_lowercase();
    let days = match unit.as_str() {
        "year" => n * 365,
        "month" => n * 30,
        "week" => n * 7,
        "day" => n,
        _ => return None,
    };
    Some(now - chrono::Duration::days(days))
}

/// Converts a libgit2 `Time` (seconds since epoch + a UTC-offset-in-minutes the commit was
/// authored/committed under) to a `DateTime<Utc>` — always normalized to UTC regardless of
/// the original commit's local offset, matching `%cI`'s ISO-8601 output re-parsed by the
/// CLI-backed `parse_date()` (which also always ends up UTC).
pub(crate) fn git2_time_to_utc(time: git2::Time) -> Option<DateTime<Utc>> {
    Utc.timestamp_opt(time.seconds(), 0).single()
}

/// `repo.statuses()`'s entries, minus any path real git would still treat as ignored
/// despite a nested `!pattern` negation re-including it. This closes a genuine libgit2
/// fidelity gap versus core git found on a real repo in this codebase's own dev machine
/// (`.gitignore` containing `.vscode/*` followed by `!.vscode/settings.json`): per
/// `gitignore(5)`, "it is not possible to re-include a file if a parent directory of that
/// file is excluded" — core git enforces this by never descending into an excluded
/// directory at all, so the negation is simply never reached. libgit2's status/ignore
/// engine does NOT enforce that rule and honors the file-level negation anyway, so
/// `Repository::statuses()` reported the file as untracked when real `git status
/// --porcelain` correctly omits it — inflating `uncommitted_files` by one on the one repo
/// that happened to exercise this pattern shape, caught only by diffing swab-rs's real-
/// $HOME output against the Python (real-git-backed) implementation.
///
/// Confirmed empirically that `Repository::status_should_ignore()`, when asked about the
/// *directory* itself (not the negated file), still correctly reports `true` — so this
/// walks each entry's ancestor directories and drops the entry if any ancestor is ignored,
/// reconstructing the "never descend into an excluded directory" rule that libgit2's own
/// tree walk skips.
pub(crate) fn effective_status_entries(repo: &git2::Repository) -> Vec<String> {
    let mut opts = git2::StatusOptions::new();
    // `recurse_untracked_dirs` is deliberately left at its default (`false`) — that default
    // is what makes libgit2 match `git status --porcelain`'s default "normal" untracked-files
    // mode, which collapses a wholly-untracked directory into ONE entry (`dir/`) rather than
    // one entry per file inside it. Setting it `true` (an earlier version of this function
    // did) is the `-uall`/`--untracked-files=all` behavior instead — a second, independent
    // libgit2-vs-porcelain divergence found the same way as the ignore-negation gap below:
    // two real repos on this codebase's own dev machine each had one wholly-untracked
    // directory, and both had their file counts inflated (every file inside counted
    // separately) versus the real Python (git-CLI-backed) implementation's single-line count.
    opts.include_untracked(true);
    let Ok(statuses) = repo.statuses(Some(&mut opts)) else {
        return Vec::new();
    };
    statuses
        .iter()
        .filter_map(|entry| entry.path().ok().map(String::from))
        .filter(|path| !ancestor_dir_is_ignored(repo, path))
        .collect()
}

/// `true` if any ancestor directory of `path` (checked as a directory, i.e. with a
/// trailing slash — libgit2, like git, only applies directory-only ignore patterns when
/// the checked path is asserted to be a directory) is itself ignored per libgit2's own
/// ignore engine. See `effective_status_entries`'s doc comment for why this check exists.
fn ancestor_dir_is_ignored(repo: &git2::Repository, path: &str) -> bool {
    let mut ancestor = std::path::Path::new(path).parent();
    while let Some(dir) = ancestor {
        if dir.as_os_str().is_empty() {
            break;
        }
        let dir_with_slash = format!("{}/", dir.display());
        if repo
            .status_should_ignore(std::path::Path::new(&dir_with_slash))
            .unwrap_or(false)
        {
            return true;
        }
        ancestor = dir.parent();
    }
    false
}

/// Parses an ISO-8601 timestamp from `text` (the exact format git emits via `%cI`).
/// Returns `None` on empty, unparseable, or malformed input — never panics.
/// The output is always timezone-aware UTC (analogous to Python's `replace(tzinfo=utc)`).
fn parse_date(text: &str) -> Option<DateTime<Utc>> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(text)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

/// Normalise a git remote URL to `https://github.com/OWNER/REPO` (with a `.git` suffix
/// stripped). Returns `None` for non-GitHub remotes, empty input, or SSH-style URLs that
/// don't point at github.com. Mirrors `petridish.git._github_url`.
pub(crate) fn github_url(remote: &str) -> Option<String> {
    let remote = remote.trim();
    if remote.is_empty() {
        return None;
    }
    let remote = remote.strip_suffix('/').unwrap_or(remote);

    if let Some(rest) = remote.strip_prefix("git@github.com:") {
        return Some(format!("https://github.com/{}", rest.strip_suffix(".git").unwrap_or(rest)));
    }

    if remote.starts_with("https://github.com/") {
        return Some(remote.strip_suffix(".git").unwrap_or(remote).to_string());
    }

    None
}

/// git2 (libgit2) implementation. Opens `path` as a repository once and reuses the handle
/// for every fact below — no process spawn anywhere. Field-for-field equivalent to the
/// CLI-subprocess version's contract (see module doc comment for the two known parsing
/// gaps: `--since` horizon and `--author` regex matching). If `path` isn't a repo at all,
/// short-circuits to `GitState::not_a_repo()` without touching any other field.
pub fn scan(path: &Path, author_patterns: &[String], author_since: &str) -> GitState {
    let repo = match git2::Repository::open(path) {
        Ok(r) => r,
        Err(_) => return GitState::not_a_repo(),
    };

    let mut result = GitState {
        is_repo: true,
        branch: None,
        is_dirty: false,
        uncommitted_files: 0,
        last_commit_at: None,
        mine_last_commit_at: None,
        github_url: None,
    };

    // Branch: "HEAD" literal for a detached checkout (matching `git rev-parse
    // --abbrev-ref HEAD`'s behavior), the branch shorthand otherwise. An unborn HEAD (fresh
    // repo, zero commits) makes `repo.head()` error — degrades to `None`, same as the CLI
    // version's `rev-parse --abbrev-ref HEAD` failing on the same repo shape.
    match repo.head_detached() {
        Ok(true) => result.branch = Some("HEAD".to_string()),
        Ok(false) => {
            if let Ok(head) = repo.head() {
                if let Ok(name) = head.shorthand() {
                    result.branch = Some(name.to_string());
                }
            }
        }
        Err(_) => {}
    }

    // Dirty state: mirrors `git status --porcelain`'s default scope (tracked modifications
    // + untracked files, ignored files excluded) via `effective_status_entries` below,
    // which additionally filters out the nested-negation-under-an-excluded-directory gap
    // libgit2's raw `statuses()` doesn't handle the way real git does.
    let entries = effective_status_entries(&repo);
    result.is_dirty = !entries.is_empty();
    result.uncommitted_files = entries.len() as u32;

    // Last commit: HEAD's committer time (matches `%cI`, which is the committer date).
    let head_commit = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    if let Some(ref commit) = head_commit {
        result.last_commit_at = git2_time_to_utc(commit.time());
    }

    // Mine last commit: for each author pattern, walk every commit reachable from HEAD,
    // keep the newest whose committer time is at/after the since-horizon AND whose
    // "Name <email>" line matches the pattern (case-insensitively) -- then take the max
    // across all patterns, exactly mirroring the CLI version's "first pattern that returns
    // a commit, keep whichever produced the latest date" loop.
    let now = Utc::now();
    let cutoff = parse_since_horizon(author_since, now);
    let mut mine_last: Option<DateTime<Utc>> = None;
    if let Ok(mut revwalk) = repo.revwalk() {
        if revwalk.push_head().is_ok() {
            for pattern in author_patterns {
                let Ok(re) = regex::Regex::new(&format!("(?i){pattern}")) else {
                    continue; // pattern doesn't compile as a regex -- skip, don't panic.
                };
                for oid in revwalk.by_ref().filter_map(|o| o.ok()) {
                    let Ok(commit) = repo.find_commit(oid) else {
                        continue;
                    };
                    let Some(commit_time) = git2_time_to_utc(commit.time()) else {
                        continue;
                    };
                    if let Some(cutoff) = cutoff {
                        if commit_time < cutoff {
                            continue;
                        }
                    }
                    let author = commit.author();
                    let signature = format!(
                        "{} <{}>",
                        author.name().unwrap_or(""),
                        author.email().unwrap_or("")
                    );
                    if re.is_match(&signature)
                        && mine_last.is_none_or(|best| commit_time > best)
                    {
                        mine_last = Some(commit_time);
                    }
                }
                // Reset the walk for the next pattern -- `revwalk` is consumed by iteration.
                let _ = revwalk.reset();
                let _ = revwalk.push_head();
            }
        }
    }
    result.mine_last_commit_at = mine_last;

    // Remote URL -> github_url. Missing remote (no "origin") degrades to None, same as the
    // CLI version's `remote get-url origin` failing on a repo with no configured remote.
    if let Ok(remote) = repo.find_remote("origin") {
        if let Ok(url) = remote.url() {
            result.github_url = github_url(url);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// Fixed author/committer dates (strict ISO 8601 with explicit +00:00).
    /// Mirrors Python's `FIXED_AUTHOR_DATE` / `GIT_AUTHOR_DATE` / `GIT_COMMITTER_DATE` approach.
    const AUTHOR_DATE: &str = "2024-01-15T10:30:00+00:00";
    const COMMITTER_DATE: &str = "2024-01-15T10:30:00+00:00";

    const GIT_ENV: &[(&str, &str)] = &[
        ("GIT_AUTHOR_DATE", AUTHOR_DATE),
        ("GIT_COMMITTER_DATE", COMMITTER_DATE),
        ("GIT_AUTHOR_NAME", "Test Author"),
        ("GIT_AUTHOR_EMAIL", "author@example.com"),
        ("GIT_COMMITTER_NAME", "Test Committer"),
        ("GIT_COMMITTER_EMAIL", "committer@example.com"),
    ];

    /// Create a fresh temp directory for the test and return its path. The caller is
    /// responsible for cleaning up (tests call `cleanup`).
    fn make_tmp_dir(name: &str) -> PathBuf {
        let tmp = std::env::temp_dir()
            .join("swab_rs_git_test")
            .join(name);
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("mktemp");
        tmp
    }

    /// Run `git init` in `dir`, returning success. Uses the pinned env above.
    fn git_init(dir: &Path) {
        assert!(
            Command::new("git")
                .args(["init", dir.to_str().unwrap()])
                .envs(GIT_ENV.iter().map(|(k, v)| (k, *v)))
                .spawn()
                .expect("git spawn")
                .wait()
                .expect("git init wait")
                .success(),
            "git init failed for {}",
            dir.display()
        );
    }

    /// Add a file and commit it in `dir`.
    fn git_add_and_commit(dir: &Path, filename: &str, content: &str) {
        let path = dir.join(filename);
        fs::write(&path, content).expect("write file");
        assert!(
            Command::new("git")
                .args([
                    "-C", dir.to_str().unwrap(), "add", filename,
                ])
                .envs(GIT_ENV.iter().map(|(k, v)| (k, *v)))
                .spawn()
                .expect("git spawn")
                .wait()
                .expect("git add wait")
                .success(),
            "git add failed for {}",
            dir.display()
        );
        assert!(
            Command::new("git")
                .args([
                    "-C", dir.to_str().unwrap(),
                    "commit", "--no-gpg-sign", "-m", "test commit", "--allow-empty",
                ])
                .envs(GIT_ENV.iter().map(|(k, v)| (k, *v)))
                .spawn()
                .expect("git spawn")
                .wait()
                .expect("git commit wait")
                .success(),
            "git commit failed for {}",
            dir.display()
        );
    }

    /// Run `git <args>` in `dir`, returning stdout as a String.
    fn git_run(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-C", dir.to_str().unwrap()])
            .args(args)
            .envs(GIT_ENV.iter().map(|(k, v)| (k, *v)))
            .output()
            .expect("git spawn")
            .stdout;
        String::from_utf8_lossy(&out).into_owned()
    }

    /// RAII guard that removes a temp dir on drop — mirrors `tempfile` semantics.
    #[derive(Debug)]
    struct TempGuard {
        path: PathBuf,
    }

    impl TempGuard {
        fn new(name: &str) -> Self {
            let path = make_tmp_dir(name);
            Self { path }
        }
    }

    impl Drop for TempGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    // 1. Non-repo path returns GitState(is_repo=false), everything else default.
    #[test]
    fn scan_non_repo_is_not_a_repo_with_defaults() {
        let tmp = make_tmp_dir("not_a_repo");

        let state = scan(&tmp, &[], "3 years");
        assert!(!state.is_repo);
        assert_eq!(state.branch, None);
        assert!(!state.is_dirty);
        assert_eq!(state.uncommitted_files, 0);
        assert_eq!(state.last_commit_at, None);
        assert_eq!(state.mine_last_commit_at, None);
        assert_eq!(state.github_url, None);
    }

    // 2. Fresh repo with one commit, clean tree.
    #[test]
    fn fresh_repo_one_commit_clean() {
        let tmp = make_tmp_dir("fresh_clean");
        git_init(&tmp);
        git_add_and_commit(&tmp, "README.md", "hello");

        let state = scan(&tmp, &[], "3 years");
        assert!(state.is_repo);
        assert!(!state.is_dirty);
        assert_eq!(state.uncommitted_files, 0);
        assert_eq!(state.last_commit_at, Some(parse_date(AUTHOR_DATE).unwrap()));
    }

    // Regression: a wholly-untracked directory (every file inside it new, nothing tracked)
    // must count as ONE uncommitted entry, matching `git status --porcelain`'s default
    // "normal" untracked-files mode (which prints a single `?? dir/` line rather than one
    // line per file). An earlier version of `effective_status_entries` set
    // `StatusOptions::recurse_untracked_dirs(true)`, which is libgit2's equivalent of
    // `--untracked-files=all` and expands the directory into every individual file --
    // caught on two real repos in this codebase's own dev machine, each inflating
    // `uncommitted_files` by (file count - 1) versus the real Python implementation.
    #[test]
    fn wholly_untracked_directory_counts_as_one_entry() {
        let tmp = make_tmp_dir("untracked_dir_collapse");
        git_init(&tmp);
        git_add_and_commit(&tmp, "README.md", "hello");

        let notes_dir = tmp.join("notes");
        fs::create_dir_all(&notes_dir).expect("mkdir notes");
        fs::write(notes_dir.join("a.md"), "a").expect("write a.md");
        fs::write(notes_dir.join("b.md"), "b").expect("write b.md");
        fs::write(notes_dir.join("c.md"), "c").expect("write c.md");

        // Sanity: confirm the real git CLI collapses this to one line, not three.
        let cli_porcelain = git_run(&tmp, &["status", "--porcelain"]);
        let cli_lines: Vec<&str> = cli_porcelain.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(
            cli_lines.len(), 1,
            "real git must collapse a wholly-untracked dir to one line: {cli_lines:?}"
        );

        let state = scan(&tmp, &[], "3 years");
        assert!(state.is_dirty);
        assert_eq!(
            state.uncommitted_files, 1,
            "a wholly-untracked directory must count as one entry, not one per file inside it"
        );
    }

    // 3. Dirty repo (uncommitted file) -> is_dirty=true, uncommitted_files>=1.
    #[test]
    fn dirty_repo_reports_uncommitted() {
        let tmp = make_tmp_dir("dirty");
        git_init(&tmp);
        git_add_and_commit(&tmp, "README.md", "hello");

        // Untracked file -> dirty.
        fs::write(tmp.join("untracked.txt"), "data").expect("write untracked");

        let state = scan(&tmp, &[], "3 years");
        assert!(state.is_repo);
        assert!(state.is_dirty, "expected dirty={}", state.is_dirty);
        assert!(
            state.uncommitted_files >= 1,
            "expected uncommitted_files>=1, got {}",
            state.uncommitted_files
        );
    }

    // Regression: a .gitignore with `dir/*` followed by `!dir/file` (negating one file
    // inside an excluded directory) must NOT surface that file as untracked -- real git
    // enforces "cannot re-include a file if a parent directory is excluded"
    // (gitignore(5)) by never descending into the excluded directory at all, but
    // libgit2's raw `Repository::statuses()` does not enforce this and returns the file
    // anyway. Caught on a real repo in this codebase's own dev machine (a large VS-style
    // .gitignore with `.vscode/*` + `!.vscode/settings.json`), where it inflated
    // `uncommitted_files` by exactly one versus the real Python (git-CLI-backed)
    // implementation.
    #[test]
    fn negated_file_inside_excluded_directory_stays_ignored() {
        let tmp = make_tmp_dir("negation_gap");
        git_init(&tmp);
        git_add_and_commit(&tmp, "README.md", "hello");

        // Must be a DIRECTORY-level exclude (trailing slash, `dir/`) to reproduce the real
        // bug -- a contents-only glob like `dir/*` (no trailing slash on `dir`) does not
        // exclude the directory itself, so git still descends into it and honors the
        // negation normally. Real git only blocks re-inclusion when the directory ITSELF
        // was excluded by a separate rule, exactly matching the real repo's `.gitignore`
        // (`.vscode/` on one line, `.vscode/*` + `!.vscode/settings.json` on later lines).
        fs::write(
            tmp.join(".gitignore"),
            "dir/\ndir/*\n!dir/keep.txt\n",
        )
        .expect("write .gitignore");
        fs::create_dir_all(tmp.join("dir")).expect("mkdir dir");
        fs::write(tmp.join("dir").join("keep.txt"), "should stay ignored").expect("write keep.txt");
        assert!(
            Command::new("git")
                .args(["-C", tmp.to_str().unwrap(), "add", ".gitignore"])
                .envs(GIT_ENV.iter().map(|(k, v)| (k, *v)))
                .spawn()
                .expect("git spawn")
                .wait()
                .expect("git add wait")
                .success(),
            "git add .gitignore failed"
        );
        assert!(
            Command::new("git")
                .args([
                    "-C", tmp.to_str().unwrap(),
                    "commit", "--no-gpg-sign", "-m", "add gitignore", "--allow-empty",
                ])
                .envs(GIT_ENV.iter().map(|(k, v)| (k, *v)))
                .spawn()
                .expect("git spawn")
                .wait()
                .expect("git commit wait")
                .success(),
            "git commit .gitignore failed"
        );

        // Sanity: confirm the real git CLI agrees this file is ignored, not just our
        // assumption about gitignore(5)'s documented behavior.
        let cli_porcelain = git_run(&tmp, &["status", "--porcelain"]);
        assert!(
            cli_porcelain.trim().is_empty(),
            "real git must treat dir/keep.txt as ignored despite the negation: {cli_porcelain:?}"
        );

        let state = scan(&tmp, &[], "3 years");
        assert!(
            !state.is_dirty,
            "dir/keep.txt must stay ignored (parent dir excluded), not surface as untracked"
        );
        assert_eq!(state.uncommitted_files, 0);
    }

    // 3b. Modified tracked file also counts as dirty.
    #[test]
    fn modified_tracked_file_reports_dirty() {
        let tmp = make_tmp_dir("modified");
        git_init(&tmp);
        git_add_and_commit(&tmp, "README.md", "original");

        // Modify the tracked file.
        fs::write(tmp.join("README.md"), "changed content").expect("rewrite tracked");

        let state = scan(&tmp, &[], "3 years");
        assert!(state.is_dirty, "expected dirty after modifying tracked file");
        assert!(state.uncommitted_files >= 1);
    }

    // 4. last_commit_at is timezone-aware UTC and matches pinned date.
    #[test]
    fn last_commit_at_is_utc_matching_pinned_date() {
        let tmp = make_tmp_dir("utc_check");
        git_init(&tmp);
        git_add_and_commit(&tmp, "README.md", "hello");

        let state = scan(&tmp, &[], "3 years");
        let expected = parse_date(AUTHOR_DATE).expect("our fixed date must parse");

        let actual = state.last_commit_at.expect("should have a commit");
        assert_eq!(actual, expected);

        // Sanity: the offset of a Utc DateTime is zero. Verify by re-anchoring to Utc and
        // confirming equality — if `actual` were anything other than Utc, re-anchoring would
        // shift the value and the equality would fail.
        assert_eq!(actual.with_timezone(&Utc), actual);
    }

    // 4b. Non-UTC pinned offset gets normalized to UTC.
    #[test]
    fn last_commit_at_converts_nonzero_offset_to_utc() {
        let tmp = make_tmp_dir("utc_offset_check");
        fs::create_dir_all(&tmp).expect("mktemp");

        let env_offset = &[
            ("GIT_AUTHOR_DATE", "2024-01-15T15:30:00+05:00"),
            ("GIT_COMMITTER_DATE", "2024-01-15T15:30:00+05:00"),
            ("GIT_AUTHOR_NAME", "Test"),
            ("GIT_AUTHOR_EMAIL", "t@t.com"),
            ("GIT_COMMITTER_NAME", "Test"),
            ("GIT_COMMITTER_EMAIL", "t@t.com"),
        ];

        assert!(Command::new("git")
            .args(["init", tmp.to_str().unwrap()])
            .envs(env_offset.iter().map(|(k, v)| (k, *v)))
            .spawn().unwrap()
            .wait().unwrap().success());

        assert!(Command::new("git")
            .args([
                "-C", tmp.to_str().unwrap(),
                "commit", "--no-gpg-sign", "-m", "x", "--allow-empty",
            ])
            .envs(env_offset.iter().map(|(k, v)| (k, *v)))
            .spawn().unwrap()
            .wait().unwrap().success());

        let state = scan(&tmp, &[], "3 years");
        // 15:30+05:00 = 10:30 UTC.
        let expected = parse_date("2024-01-15T10:30:00+00:00").unwrap();
        assert_eq!(
            state.last_commit_at,
            Some(expected),
            "non-UTC offset must be normalized to UTC"
        );

        // Re-anchor to Utc and confirm equality — proves we ended up in Utc.
        assert_eq!(state.last_commit_at.unwrap().with_timezone(&Utc), state.last_commit_at.unwrap());
    }

    // 5. Empty repo (git init, zero commits): doesn't panic, is_repo=true, last_commit_at=None.
    #[test]
    fn empty_repo_no_commits_does_not_panic() {
        let tmp = make_tmp_dir("empty_repo");
        git_init(&tmp);

        let state = scan(&tmp, &[], "3 years");
        assert!(state.is_repo, "empty repo should still be a repo");
        assert_eq!(state.last_commit_at, None, "no commits -> last_commit_at is None");
        assert_eq!(state.mine_last_commit_at, None);
    }

    // 6. Author pattern matches last commit's author -> mine_last_commit_at set.
    #[test]
    fn author_pattern_matches_sets_mine() {
        let tmp = make_tmp_dir("author_match");
        git_init(&tmp);

        // Specific author via env for the commit.
        let env_author = &[
            ("GIT_AUTHOR_DATE", AUTHOR_DATE),
            ("GIT_COMMITTER_DATE", COMMITTER_DATE),
            ("GIT_AUTHOR_NAME", "Jan Krag"),
            ("GIT_AUTHOR_EMAIL", "j@krag.com"),
            ("GIT_COMMITTER_NAME", "Jan Krag"),
            ("GIT_COMMITTER_EMAIL", "j@krag.com"),
        ];
        assert!(Command::new("git")
            .args(["-C", tmp.to_str().unwrap(),
                "commit", "--no-gpg-sign", "-m", "mine", "--allow-empty"])
            .envs(env_author.iter().map(|(k, v)| (k, *v)))
            .spawn().unwrap()
            .wait().unwrap().success());

        let state = scan(&tmp, &["Jan Krag".to_string()], "3 years");
        assert!(
            state.mine_last_commit_at.is_some(),
            "pattern matched but mine_last_commit_at was None"
        );
        assert_eq!(
            state.mine_last_commit_at,
            Some(parse_date(AUTHOR_DATE).unwrap())
        );
    }

    // 7. Author pattern does NOT match -> mine_last_commit_at stays None.
    #[test]
    fn author_pattern_no_match_leaves_mine_none() {
        let tmp = make_tmp_dir("author_nomatch");
        git_init(&tmp);

        // Commit with a specific author.
        let env_author = &[
            ("GIT_AUTHOR_DATE", AUTHOR_DATE),
            ("GIT_COMMITTER_DATE", COMMITTER_DATE),
            ("GIT_AUTHOR_NAME", "Other Person"),
            ("GIT_AUTHOR_EMAIL", "o@other.com"),
            ("GIT_COMMITTER_NAME", "Other Person"),
            ("GIT_COMMITTER_EMAIL", "o@other.com"),
        ];
        assert!(Command::new("git")
            .args(["-C", tmp.to_str().unwrap(),
                "commit", "--no-gpg-sign", "-m", "mine", "--allow-empty"])
            .envs(env_author.iter().map(|(k, v)| (k, *v)))
            .spawn().unwrap()
            .wait().unwrap().success());

        // Look for a different author; should not match.
        let state = scan(&tmp, &["Jan Krag".to_string()], "3 years");
        assert_eq!(state.mine_last_commit_at, None);
    }

    // 8. SSH remote normalizes to HTTPS.
    #[test]
    fn ssh_remote_normalizes_to_github_https() {
        let tmp = make_tmp_dir("ssh_remote");
        git_init(&tmp);

        git_run(&tmp, &["remote", "add", "origin", "git@github.com:OWNER/REPO.git"]);

        let state = scan(&tmp, &[], "3 years");
        assert_eq!(
            state.github_url,
            Some("https://github.com/OWNER/REPO".to_string()),
            "ssh form should normalize to bare HTTPS URL"
        );
    }

    // 9. HTTPS remote normalizes to bare HTTPS (strip .git).
    #[test]
    fn https_remote_normalizes_to_github_https() {
        let tmp = make_tmp_dir("https_remote");
        git_init(&tmp);

        git_run(&tmp, &["remote", "add", "origin", "https://github.com/OWNER/REPO.git"]);

        let state = scan(&tmp, &[], "3 years");
        assert_eq!(
            state.github_url,
            Some("https://github.com/OWNER/REPO".to_string()),
            "https form should strip .git suffix"
        );
    }

    // 10. Non-GitHub remote -> github_url None.
    #[test]
    fn non_github_remote_yields_none() {
        let tmp = make_tmp_dir("gitlab_remote");
        git_init(&tmp);

        git_run(
            &tmp,
            &["remote", "add", "origin", "https://gitlab.com/OWNER/REPO.git"],
        );

        let state = scan(&tmp, &[], "3 years");
        assert_eq!(
            state.github_url, None,
            "gitlab remote must not produce a github URL"
        );
    }

    // 11. No remote configured -> github_url None (is_repo still true).
    #[test]
    fn no_remote_yields_none() {
        let tmp = make_tmp_dir("no_remote");
        git_init(&tmp);
        git_add_and_commit(&tmp, "README.md", "hello");

        let state = scan(&tmp, &[], "3 years");
        assert!(state.is_repo);
        assert_eq!(state.github_url, None);
    }

    // 12. Detached HEAD (checkout a commit SHA directly) -> doesn't panic.
    #[test]
    fn detached_head_does_not_panic() {
        let tmp = make_tmp_dir("detached");
        git_init(&tmp);
        git_add_and_commit(&tmp, "README.md", "hello");

        let head_sha = git_run(&tmp, &["rev-parse", "HEAD"]).trim().to_string();
        assert!(Command::new("git")
            .args(["-C", tmp.to_str().unwrap(), "checkout", &head_sha])
            .spawn().unwrap()
            .wait().unwrap().success());

        // Should not panic; branch will read "HEAD" in detached state (parity with Python).
        let state = scan(&tmp, &[], "3 years");
        assert!(state.is_repo);
        assert_eq!(state.branch.as_deref(), Some("HEAD"));
        assert!(state.last_commit_at.is_some());
    }

    // github_url() unit tests — pure logic, no git invocations.
    #[test]
    fn github_url_ssh_normalization() {
        assert_eq!(
            github_url("git@github.com:OWNER/REPO.git"),
            Some("https://github.com/OWNER/REPO".to_string())
        );
    }

    #[test]
    fn github_url_https_normalization() {
        assert_eq!(
            github_url("https://github.com/OWNER/REPO.git"),
            Some("https://github.com/OWNER/REPO".to_string())
        );
    }

    #[test]
    fn github_url_ssh_without_dotgit() {
        assert_eq!(
            github_url("git@github.com:OWNER/REPO"),
            Some("https://github.com/OWNER/REPO".to_string())
        );
    }

    #[test]
    fn github_url_https_without_dotgit() {
        assert_eq!(
            github_url("https://github.com/OWNER/REPO"),
            Some("https://github.com/OWNER/REPO".to_string())
        );
    }

    #[test]
    fn github_url_non_github_returns_none() {
        assert_eq!(github_url("https://gitlab.com/OWNER/REPO.git"), None);
        assert_eq!(
            github_url("ssh://git@gitlab.com/OWNER/REPO.git"),
            None
        );
        assert_eq!(
            github_url("https://bitbucket.org/OWNER/REPO.git"),
            None
        );
    }

    #[test]
    fn github_url_empty_returns_none() {
        assert_eq!(github_url(""), None);
        assert_eq!(github_url("  "), None);
    }

    #[test]
    fn parse_date_rejects_empty_and_invalid() {
        assert_eq!(parse_date(""), None);
        assert_eq!(parse_date("   "), None);
        assert_eq!(parse_date("not-a-date"), None);
    }

    #[test]
    fn parse_date_accepts_rfc3339() {
        let parsed = parse_date("2024-01-15T10:30:00+00:00").unwrap();
        assert_eq!(parsed.year(), 2024);
        assert_eq!(parsed.month(), 1);
        assert_eq!(parsed.day(), 15);
        assert_eq!(parsed.hour(), 10);
        assert_eq!(parsed.minute(), 30);
    }

    #[test]
    fn parse_date_accepts_z_suffix() {
        let parsed = parse_date("2024-01-15T10:30:00Z").unwrap();
        // Z suffix must resolve to Utc — re-anchor to Utc and confirm equality.
        assert_eq!(parsed.with_timezone(&Utc), parsed);
    }

    #[test]
    fn parse_date_rejects_non_iso() {
        // RFC 2822-style date is NOT strict ISO 8601.
        assert!(parse_date("Mon, 15 Jan 2024 10:30:00 +0000").is_none());
    }

    // A scan call with a nonexistent path should not panic.
    #[test]
    fn scan_nonexistent_path_does_not_panic() {
        let _guard = TempGuard::new("scan_nonexistent");
        // Use a path under /nonexistent so we never collide with anything real.
        scan(Path::new("/swab_rs_no_such_dir_xyzzy99"), &[], "3 years");
    }

    // All tests cleaned up via TempGuard drop impl. No global teardown needed — Rust
    // guarantees Drop runs when the guard goes out of scope at end of each test.
}
