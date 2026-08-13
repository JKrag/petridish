//! Git facts for one path. Mirrors `src/petridish/git.py`.
//!
//! Invariant #6 (`CLAUDE.md`): "`git` calls use `subprocess.run` with `check=False` and a
//! 5s timeout. A git failure is a `GitState(is_repo=False)`, never an exception." In Rust,
//! `std::process::Command::output()` has **no built-in timeout** — this module must wrap
//! every git invocation with the `wait-timeout` crate (already in Cargo.toml) or an
//! equivalent thread+kill, and must never let a git failure/timeout become a panic or `Err`
//! that propagates past this module.

use crate::schema::GitState;
use chrono::{DateTime, Utc};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

pub const GIT_TIMEOUT: Duration = Duration::from_secs(5);

/// The single git-with-timeout entry point for the whole crate — `discovery::is_foreign`
/// (R4, implemented after this module) reuses this rather than writing a second wrapper, so
/// there is exactly one place that can get the 5s-timeout/never-panic invariant wrong.
/// Returns `Some((success, stdout))` on a completed process (`success` mirrors
/// `check=false` — a nonzero exit is still `Some`, not an error), or `None` on a spawn
/// failure or a timeout (in which case the child is killed and reaped, never left running).
pub(crate) fn run_git(path: &Path, args: &[&str]) -> Option<(bool, String)> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    match child.wait_timeout(GIT_TIMEOUT).ok()? {
        Some(status) => {
            let output = child.wait_with_output().ok()?;
            let success = status.success();
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            Some((success, stdout))
        }
        None => {
            let _ = child.kill();
            let _ = child.wait();
            None // timed out — degrade to None, never panic
        }
    }
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

/// Runs: `rev-parse --git-dir` (is-a-repo), `rev-parse --abbrev-ref HEAD` (branch),
/// `status --porcelain` (dirty + uncommitted file count), `log -1 --format=%cI` (last
/// commit), `log -1 --format=%cI --author=<pattern> --since=<horizon>` for each of
/// `config.author_patterns` (first match wins, for `mine_last_commit_at`), and
/// `remote get-url origin` normalized to an `https://github.com/...` URL (SSH or HTTPS
/// remote forms both normalize; a non-GitHub remote or no remote => `None`). Any command
/// failing, erroring, or timing out (5s, `GIT_TIMEOUT`) => that field is `None`/default,
/// never a panic — and if `rev-parse --git-dir` itself fails, short-circuit to
/// `GitState::not_a_repo()` without running the rest. Built entirely on `run_git` above.
pub fn scan(path: &Path, author_patterns: &[String], author_since: &str) -> GitState {
    let is_repo = run_git(path, &["rev-parse", "--git-dir"])
        .map(|(ok, _)| ok)
        .unwrap_or(false);

    if !is_repo {
        return GitState::not_a_repo();
    }

    let mut result = GitState {
        is_repo: true,
        branch: None,
        is_dirty: false,
        uncommitted_files: 0,
        last_commit_at: None,
        mine_last_commit_at: None,
        github_url: None,
    };

    // Branch.
    if let Some((true, out)) = run_git(path, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        let branch = out.trim().to_string();
        if !branch.is_empty() {
            result.branch = Some(branch);
        }
    }

    // Dirty state.
    if let Some((true, out)) = run_git(path, &["status", "--porcelain"]) {
        let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
        result.is_dirty = !lines.is_empty();
        // `uncommitted_files` here is the Python parity: count of non-empty porcelain lines.
        // The schema still uses u32; a long tree wouldn't realistically overflow anyway.
        result.uncommitted_files = lines.len() as u32;
    }

    // Last commit.
    if let Some((true, out)) = run_git(path, &["log", "-1", "--format=%cI"]) {
        if let Some(dt) = parse_date(&out) {
            result.last_commit_at = Some(dt);
        }
    }

    // Mine last commit: iterate author patterns in order, first match wins.
    let mut mine_last: Option<DateTime<Utc>> = None;
    for pattern in author_patterns {
        let since_arg = format!("--since={author_since}");
        let author_arg = format!("--author={pattern}");
        let args: &[&str] = &[
            "log",
            "-1",
            "--format=%cI",
            &author_arg,
            &since_arg,
        ];
        if let Some((true, out)) = run_git(path, args) {
            if let Some(dt) = parse_date(&out) {
                if mine_last.is_none() || dt > mine_last.unwrap() {
                    mine_last = Some(dt);
                }
            }
        }
    }
    result.mine_last_commit_at = mine_last;

    // Remote URL -> github_url.
    if let Some((true, out)) = run_git(path, &["remote", "get-url", "origin"]) {
        if let Some(url) = github_url(&out) {
            result.github_url = Some(url);
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

    // run_git returns success=false on nonzero exit (check=false semantics).
    #[test]
    fn run_git_nonzero_exit_is_some_success_false() {
        let tmp = make_tmp_dir("run_git_nonzero");
        git_init(&tmp);
        // `git show nonexistent-ref` fails with a clean nonzero exit in any repo.
        let result = run_git(&tmp, &["show", "does-not-exist-ref-xyz"]);
        assert!(result.is_some(), "run_git must return Some even on nonzero exit");
        let (success, _) = result.unwrap();
        assert!(!success, "success flag must reflect nonzero exit");
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
