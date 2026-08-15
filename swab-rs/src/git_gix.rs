//! Git facts for one path, via `gix` (pure-Rust, no libgit2/C dependency).
//!
//! ## EXPERIMENTAL: gix backend spike, `experiment/gitoxide-backend`
//!
//! Sibling module to `git.rs`'s git2-hybrid backend, NOT a replacement for it — `git.rs` is
//! left untouched as the comparison baseline. Same field-for-field contract as `git.rs`'s
//! `scan()`: same `GitState`, same `not_a_repo()` fallback, same invariant #6 (a git failure
//! degrades to `is_repo: false` / `None` fields, never panics or propagates an exception).
//!
//! Motivation (full history in `git.rs`'s own module doc comment): the git2-hybrid backend
//! needed to keep ONE query — `mine_last_commit_at`'s `--author --since` search — on a CLI
//! subprocess, because git2's revwalk (parsing a full commit object per step via
//! `find_commit()`, no commit-graph acceleration) was measurably slower per commit visited
//! than git's own C search, especially when the pattern doesn't match and the walk can't
//! early-exit. The open question this module exists to answer: does gix's memory-mapped,
//! lazy-parse object access (and its own commit-graph support) let its revwalk close that
//! gap in-process, avoiding a CLI subprocess call entirely?
//!
//! Whether this module's status/ignore handling has the same two libgit2-vs-real-git parity
//! gaps found and fixed in `git.rs` (negated-file-inside-excluded-directory,
//! wholly-untracked-directory-collapse) is NOT assumed — gix's status/ignore engine is an
//! independent implementation. Both are re-verified here with the same regression tests
//! ported from `git.rs`, not inherited.

use crate::schema::GitState;
use chrono::{DateTime, TimeZone, Utc};
use std::path::Path;

/// Parses an ISO-8601 timestamp — identical contract to `git::parse_date` (kept as a
/// private copy here rather than shared, since this module must stand alone as a
/// self-contained comparison backend, not reach into `git.rs`).
fn parse_date(text: &str) -> Option<DateTime<Utc>> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(text)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

/// Normalise a git remote URL to `https://github.com/OWNER/REPO` — identical logic/contract
/// to `git::github_url`, duplicated for the same self-containment reason as `parse_date`.
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

/// gix implementation. Field-for-field equivalent to `git::scan`'s contract. If `path`
/// isn't a repo at all, short-circuits to `GitState::not_a_repo()`.
pub fn scan(path: &Path, author_patterns: &[String], author_since: &str) -> GitState {
    let repo = match gix::open(path) {
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

    result.branch = branch_name(&repo);

    let entries = status_entries(&repo);
    result.is_dirty = !entries.is_empty();
    result.uncommitted_files = entries.len() as u32;

    if let Ok(head_commit) = repo.head_commit() {
        if let Ok(time) = head_commit.time() {
            result.last_commit_at = gix_time_to_utc(time);
        }
    }

    let mut mine_last: Option<DateTime<Utc>> = None;
    for pattern in author_patterns {
        if let Some(dt) = author_since_revwalk(&repo, pattern, author_since) {
            if mine_last.is_none_or(|best| dt > best) {
                mine_last = Some(dt);
            }
        }
    }
    result.mine_last_commit_at = mine_last;

    if let Some(Ok(remote)) = repo.find_default_remote(gix::remote::Direction::Fetch) {
        if let Some(url) = remote.url(gix::remote::Direction::Fetch) {
            result.github_url = github_url(&url.to_bstring().to_string());
        }
    }

    result
}

fn branch_name(repo: &gix::Repository) -> Option<String> {
    let head = repo.head().ok()?;
    if head.is_detached() {
        return Some("HEAD".to_string());
    }
    head.referent_name().map(|n| n.shorten().to_string())
}

fn gix_time_to_utc(time: gix::date::Time) -> Option<DateTime<Utc>> {
    Utc.timestamp_opt(time.seconds, 0).single()
}

fn status_entries(repo: &gix::Repository) -> Vec<String> {
    let Ok(platform) = repo.status(gix::progress::Discard) else {
        return Vec::new();
    };
    let Ok(iter) = platform.into_iter(None) else {
        return Vec::new();
    };
    iter.filter_map(|item| item.ok())
        .map(|item| item.location().to_string())
        .collect()
}

fn author_since_revwalk(repo: &gix::Repository, pattern: &str, since: &str) -> Option<DateTime<Utc>> {
    let head_id = repo.head_id().ok()?;
    let since_dt = parse_since(since)?;
    let walk = repo.rev_walk([head_id]).all().ok()?;
    for info in walk.filter_map(|i| i.ok()) {
        let commit = info.object().ok()?;
        let commit_time = gix_time_to_utc(commit.time().ok()?)?;
        if commit_time < since_dt {
            break;
        }
        let author = commit.author().ok()?;
        let author_str = format!("{} <{}>", author.name, author.email);
        if author_str.contains(pattern) || author.name.to_string().contains(pattern) {
            return Some(commit_time);
        }
    }
    None
}

/// Parses git's `--since=<N> years|months|days|weeks` relative-date grammar into an
/// absolute cutoff. Only the subset `swab`'s config actually emits is supported.
fn parse_since(since: &str) -> Option<DateTime<Utc>> {
    let parts: Vec<&str> = since.trim().split_whitespace().collect();
    let [n, unit] = parts.as_slice() else { return None };
    let n: i64 = n.parse().ok()?;
    let unit = unit.trim_end_matches('s');
    let now = Utc::now();
    match unit {
        "year" => Some(now - chrono::Duration::days(365 * n)),
        "month" => Some(now - chrono::Duration::days(30 * n)),
        "week" => Some(now - chrono::Duration::weeks(n)),
        "day" => Some(now - chrono::Duration::days(n)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

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

    fn make_tmp_dir(name: &str) -> PathBuf {
        let tmp = std::env::temp_dir().join("swab_rs_git_gix_test").join(name);
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("mktemp");
        tmp
    }

    fn git_init(dir: &Path) {
        assert!(Command::new("git")
            .args(["init", dir.to_str().unwrap()])
            .envs(GIT_ENV.iter().map(|(k, v)| (k, *v)))
            .spawn().expect("git spawn")
            .wait().expect("git init wait")
            .success());
    }

    fn git_add_and_commit(dir: &Path, filename: &str, content: &str) {
        let path = dir.join(filename);
        fs::write(&path, content).expect("write file");
        assert!(Command::new("git")
            .args(["-C", dir.to_str().unwrap(), "add", filename])
            .envs(GIT_ENV.iter().map(|(k, v)| (k, *v)))
            .spawn().expect("git spawn")
            .wait().expect("git add wait")
            .success());
        assert!(Command::new("git")
            .args(["-C", dir.to_str().unwrap(), "commit", "--no-gpg-sign", "-m", "test commit", "--allow-empty"])
            .envs(GIT_ENV.iter().map(|(k, v)| (k, *v)))
            .spawn().expect("git spawn")
            .wait().expect("git commit wait")
            .success());
    }

    fn git_run(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-C", dir.to_str().unwrap()])
            .args(args)
            .envs(GIT_ENV.iter().map(|(k, v)| (k, *v)))
            .output().expect("git spawn").stdout;
        String::from_utf8_lossy(&out).into_owned()
    }

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

    #[test]
    fn dirty_repo_reports_uncommitted() {
        let tmp = make_tmp_dir("dirty");
        git_init(&tmp);
        git_add_and_commit(&tmp, "README.md", "hello");
        fs::write(tmp.join("untracked.txt"), "data").expect("write untracked");

        let state = scan(&tmp, &[], "3 years");
        assert!(state.is_dirty);
        assert!(state.uncommitted_files >= 1);
    }

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

        let cli_porcelain = git_run(&tmp, &["status", "--porcelain"]);
        let cli_lines: Vec<&str> = cli_porcelain.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(cli_lines.len(), 1, "real git must collapse a wholly-untracked dir to one line: {cli_lines:?}");

        let state = scan(&tmp, &[], "3 years");
        assert!(state.is_dirty);
        assert_eq!(
            state.uncommitted_files, 1,
            "gix status parity check: a wholly-untracked directory must count as one entry, not one per file"
        );
    }

    #[test]
    fn negated_file_inside_excluded_directory_stays_ignored() {
        let tmp = make_tmp_dir("negation_gap");
        git_init(&tmp);
        git_add_and_commit(&tmp, "README.md", "hello");

        fs::write(tmp.join(".gitignore"), "dir/\ndir/*\n!dir/keep.txt\n").expect("write .gitignore");
        fs::create_dir_all(tmp.join("dir")).expect("mkdir dir");
        fs::write(tmp.join("dir").join("keep.txt"), "should stay ignored").expect("write keep.txt");
        assert!(Command::new("git")
            .args(["-C", tmp.to_str().unwrap(), "add", ".gitignore"])
            .envs(GIT_ENV.iter().map(|(k, v)| (k, *v)))
            .spawn().unwrap().wait().unwrap().success());
        assert!(Command::new("git")
            .args(["-C", tmp.to_str().unwrap(), "commit", "--no-gpg-sign", "-m", "add gitignore", "--allow-empty"])
            .envs(GIT_ENV.iter().map(|(k, v)| (k, *v)))
            .spawn().unwrap().wait().unwrap().success());

        let cli_porcelain = git_run(&tmp, &["status", "--porcelain"]);
        assert!(cli_porcelain.trim().is_empty(), "real git must treat dir/keep.txt as ignored despite the negation");

        let state = scan(&tmp, &[], "3 years");
        assert!(
            !state.is_dirty,
            "gix status parity check: dir/keep.txt must stay ignored (parent dir excluded), not surface as untracked"
        );
        assert_eq!(state.uncommitted_files, 0);
    }

    #[test]
    fn empty_repo_no_commits_does_not_panic() {
        let tmp = make_tmp_dir("empty_repo");
        git_init(&tmp);

        let state = scan(&tmp, &[], "3 years");
        assert!(state.is_repo);
        assert_eq!(state.last_commit_at, None);
        assert_eq!(state.mine_last_commit_at, None);
    }

    #[test]
    fn author_pattern_matches_sets_mine() {
        let tmp = make_tmp_dir("author_match");
        git_init(&tmp);

        let env_author = &[
            ("GIT_AUTHOR_DATE", AUTHOR_DATE),
            ("GIT_COMMITTER_DATE", COMMITTER_DATE),
            ("GIT_AUTHOR_NAME", "Jan Krag"),
            ("GIT_AUTHOR_EMAIL", "j@krag.com"),
            ("GIT_COMMITTER_NAME", "Jan Krag"),
            ("GIT_COMMITTER_EMAIL", "j@krag.com"),
        ];
        assert!(Command::new("git")
            .args(["-C", tmp.to_str().unwrap(), "commit", "--no-gpg-sign", "-m", "mine", "--allow-empty"])
            .envs(env_author.iter().map(|(k, v)| (k, *v)))
            .spawn().unwrap().wait().unwrap().success());

        let state = scan(&tmp, &["Jan Krag".to_string()], "3 years");
        assert!(state.mine_last_commit_at.is_some());
        assert_eq!(state.mine_last_commit_at, Some(parse_date(AUTHOR_DATE).unwrap()));
    }

    #[test]
    fn author_pattern_no_match_leaves_mine_none() {
        let tmp = make_tmp_dir("author_nomatch");
        git_init(&tmp);

        let env_author = &[
            ("GIT_AUTHOR_DATE", AUTHOR_DATE),
            ("GIT_COMMITTER_DATE", COMMITTER_DATE),
            ("GIT_AUTHOR_NAME", "Other Person"),
            ("GIT_AUTHOR_EMAIL", "o@other.com"),
            ("GIT_COMMITTER_NAME", "Other Person"),
            ("GIT_COMMITTER_EMAIL", "o@other.com"),
        ];
        assert!(Command::new("git")
            .args(["-C", tmp.to_str().unwrap(), "commit", "--no-gpg-sign", "-m", "mine", "--allow-empty"])
            .envs(env_author.iter().map(|(k, v)| (k, *v)))
            .spawn().unwrap().wait().unwrap().success());

        let state = scan(&tmp, &["Jan Krag".to_string()], "3 years");
        assert_eq!(state.mine_last_commit_at, None);
    }

    #[test]
    fn ssh_remote_normalizes_to_github_https() {
        let tmp = make_tmp_dir("ssh_remote");
        git_init(&tmp);
        git_run(&tmp, &["remote", "add", "origin", "git@github.com:OWNER/REPO.git"]);

        let state = scan(&tmp, &[], "3 years");
        assert_eq!(state.github_url, Some("https://github.com/OWNER/REPO".to_string()));
    }

    #[test]
    fn non_github_remote_yields_none() {
        let tmp = make_tmp_dir("gitlab_remote");
        git_init(&tmp);
        git_run(&tmp, &["remote", "add", "origin", "https://gitlab.com/OWNER/REPO.git"]);

        let state = scan(&tmp, &[], "3 years");
        assert_eq!(state.github_url, None);
    }

    #[test]
    fn detached_head_does_not_panic() {
        let tmp = make_tmp_dir("detached");
        git_init(&tmp);
        git_add_and_commit(&tmp, "README.md", "hello");

        let head_sha = git_run(&tmp, &["rev-parse", "HEAD"]).trim().to_string();
        assert!(Command::new("git")
            .args(["-C", tmp.to_str().unwrap(), "checkout", &head_sha])
            .spawn().unwrap().wait().unwrap().success());

        let state = scan(&tmp, &[], "3 years");
        assert!(state.is_repo);
        assert_eq!(state.branch.as_deref(), Some("HEAD"));
        assert!(state.last_commit_at.is_some());
    }

    #[test]
    fn github_url_ssh_normalization() {
        assert_eq!(github_url("git@github.com:OWNER/REPO.git"), Some("https://github.com/OWNER/REPO".to_string()));
    }

    #[test]
    fn github_url_empty_returns_none() {
        assert_eq!(github_url(""), None);
    }
}
