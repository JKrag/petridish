//! Git facts for one path, via `gix` (pure-Rust, no libgit2/C dependency, no subprocess).
//!
//! Mirrors `src/petridish/git.py`'s contract field-for-field: same `GitState`, same
//! `not_a_repo()` fallback, same invariant #6 (a git failure degrades to `is_repo: false` /
//! `None` fields, never panics or propagates an exception).
//!
//! ## Provenance
//!
//! This module went through two prior backends before landing here, each replaced after a
//! real measurement, not a guess — see git history on `experiment/git2-backend` and
//! `experiment/gitoxide-backend` for the full diagnostic trail:
//!
//! 1. **Pure CLI subprocess** (the original port): correct, but wall-clock time was
//!    dominated by OS `sys` time (process fork/exec overhead) rather than actual git work.
//! 2. **git2 (libgit2) hybrid**: moved `is_repo`/`branch`/`status`/`last_commit_at`/`remote`
//!    to git2 (in-process, no process spawn), ~2.25x faster overall on a clean benchmark.
//!    But `mine_last_commit_at`'s `--author --since` search stayed on a CLI subprocess call,
//!    because git2's revwalk (parsing a full commit object per step via `find_commit()`, no
//!    commit-graph acceleration) was measurably slower per commit visited than git's own
//!    optimized CLI search — confirmed by isolating the revwalk via diagnostic instrumentation:
//!    removing it alone dropped `user` CPU time from ~3.2s to ~1.87s on an 80-repo scan.
//! 3. **gix (this module)**: gix's revwalk closes that gap in-process — a clean 5-run
//!    benchmark on the real 80-project dev `$HOME` (no CLI subprocess anywhere) measured
//!    ~1.38s avg wall-clock vs git2-hybrid's ~2.25s and the original CLI-subprocess port's
//!    ~4.82s, with LOWER `user` CPU time than the git2-hybrid (0.58s vs 0.86s) despite doing
//!    the author/since search fully in-process — gix's revwalk is genuinely more efficient
//!    per commit visited, not just avoiding process-spawn overhead. `git2` and `wait-timeout`
//!    were removed from `Cargo.toml` once this became the sole backend.
//!
//! Whether this module's status/ignore engine reproduced the two libgit2-vs-real-git parity
//! gaps found in the git2 backend (`.gitignore` negation inside an excluded directory,
//! wholly-untracked-directory collapse) was independently re-verified, not assumed — neither
//! reproduces here; gix's status/ignore engine is a separate implementation. Three new,
//! gix-specific parity gaps WERE found and fixed (each via `diff_check.sh` against real
//! fixtures or the real dev `$HOME`, with a regression test): an unborn HEAD's branch name
//! (`repo.head()`/`referent_name()` can resolve a name real `git rev-parse --abbrev-ref HEAD`
//! refuses to, since that CLI command fails outright pre-first-commit), `author_patterns`
//! being a regular expression rather than a literal substring, a `url.*.insteadOf`
//! global-config shorthand remote (gix's `Remote` type requires the raw value to parse as a
//! URL before rewriting; real git/libgit2 rewrite the raw string first, independent of
//! validity), and a staged-then-further-modified file being counted twice (gix's status
//! stream emits two items for what `git status --porcelain` collapses into one `AM` line).

use crate::schema::GitState;
use chrono::{DateTime, TimeZone, Utc};
use std::path::Path;

/// Parses an ISO-8601 timestamp from `text`. Returns `None` on empty, unparseable, or
/// malformed input — never panics. The output is always timezone-aware UTC.
///
/// `#[cfg(test)]`: only ever called from this module's own tests (as a fixture-date-string
/// parser to build expected values) — pre-existing dead code in production builds, not
/// introduced by any of this branch's changes. Gated rather than deleted since deleting it
/// would just mean re-writing the same test helper under a different name.
#[cfg(test)]
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

/// gix implementation of `git::scan`. If `path` isn't a repo at all, short-circuits to
/// `GitState::not_a_repo()` without touching any other field.
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
        daily_commits: Vec::new(),
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

    if let Some(raw) = resolve_remote_fetch_url(&repo) {
        result.github_url = github_url(&raw);
    }

    result.daily_commits = daily_commit_counts(&repo, Utc::now());

    result
}

/// Buckets commits from HEAD into `GIT_ACTIVITY_WINDOW_DAYS` trailing daily counts, oldest
/// first, today last. Recomputed fresh every call (unlike `Project::agent_activity`, git
/// already retains its own commit history so there's nothing to carry forward).
///
/// Bounded by `gix`'s own `ByCommitTimeCutoff` traversal rather than by breaking out of a
/// default walk at the first old commit. That earlier form assumed revwalk output is
/// strictly commit-time-descending, and its own comment admitted the assumption was "an
/// approximation, not a guarantee (merge commits can interleave)" — which is precisely a
/// wrong answer waiting for a history to produce it. A merge whose commit date predates
/// an in-window commit reachable behind it ended the walk early, and the sparkline then
/// published a count that was simply too low, silently and plausibly. `ByCommitTimeCutoff`
/// prioritises by commit time and stops only once nothing younger than the cutoff remains
/// queued, so the bound is a property of the traversal instead of a guess about its order.
pub(crate) fn daily_commit_counts(repo: &gix::Repository, now: DateTime<Utc>) -> Vec<u32> {
    let window = crate::schema::GIT_ACTIVITY_WINDOW_DAYS;
    let mut buckets = vec![0u32; window];

    let Ok(head_id) = repo.head_id() else { return buckets };

    let today = now.date_naive();
    let window_start = today - chrono::Duration::days(window as i64 - 1);
    // Midnight at the start of the window, as seconds since the epoch — the cutoff the
    // traversal itself enforces. Anything the walk still yields below this (the cutoff
    // prunes the queue, it does not filter each item) is dropped by the guard below.
    let cutoff_seconds = window_start
        .and_hms_opt(0, 0, 0)
        .map(|dt| dt.and_utc().timestamp())
        .unwrap_or(0);

    let Ok(walk) = repo
        .rev_walk([head_id])
        .sorting(gix::revision::walk::Sorting::ByCommitTimeCutoff {
            order: gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            seconds: cutoff_seconds,
        })
        .all()
    else {
        return buckets;
    };

    for info in walk.filter_map(|i| i.ok()) {
        let Ok(commit) = info.object() else { continue };
        let Ok(time) = commit.time() else { continue };
        let Some(commit_time) = gix_time_to_utc(time) else { continue };
        let commit_date = commit_time.date_naive();
        if commit_date < window_start {
            // `continue`, not `break`: the cutoff bounds how far the traversal goes, and
            // this only discards a straggler it still handed us. Breaking here would
            // reintroduce the ordering assumption the cutoff exists to remove.
            continue;
        }
        // A commit dated in the future (clock skew) clamps into today's bucket rather than
        // panicking on a negative index.
        let days_ago = (today - commit_date).num_days().clamp(0, window as i64 - 1);
        let idx = (window as i64 - 1 - days_ago) as usize;
        buckets[idx] += 1;
    }

    buckets
}

/// Mirrors `git rev-parse --abbrev-ref HEAD`'s exact behavior, including its one surprising
/// case: on an unborn HEAD (fresh repo, zero commits) the CLI command fails outright ("fatal:
/// ambiguous argument 'HEAD': unknown revision", exit 128) even though HEAD symbolically
/// points at a named branch — confirmed empirically against real git. gix's
/// `referent_name()` returns `Some("refs/heads/master")` for this same unborn case (it CAN
/// resolve the symbolic name, git's plumbing chooses not to), so the unborn case is
/// special-cased to `None` here to match the CLI.
fn branch_name(repo: &gix::Repository) -> Option<String> {
    let head = repo.head().ok()?;
    if head.is_detached() {
        return Some("HEAD".to_string());
    }
    if matches!(head.kind, gix::head::Kind::Unborn(_)) {
        return None;
    }
    head.referent_name().map(|n| n.shorten().to_string())
}

fn gix_time_to_utc(time: gix::date::Time) -> Option<DateTime<Utc>> {
    Utc.timestamp_opt(time.seconds, 0).single()
}

/// `remote.origin.url`, with `url.<base>.insteadOf` rewriting applied by hand.
///
/// Found via `diff_check.sh` against real `$HOME` (a repo whose `remote.origin.url` is the
/// literal shorthand string `"gk:"`, rewritten to a real GitHub URL only via a global
/// `[url "https://github.com/eficode-academy/git-katas.git"] insteadOf = gk:` entry). Real
/// git and libgit2 both apply `insteadOf` as a pure textual prefix-substitution BEFORE
/// treating the result as a URL at all — the raw value never has to be a valid URL on its
/// own. `gix::Repository::find_remote`/`find_default_remote` do NOT do this: they refuse to
/// construct a `Remote` at all once the raw value fails `gix_url::parse` (confirmed via a
/// throwaway probe — `try_find_remote_without_url_rewrite` errors identically, so this
/// isn't a rewrite-ordering bug, gix's `Remote` construction pipeline requires URL-validity
/// up front where git's does not). Reimplemented by hand here directly against the raw
/// config string + `url.*.insteadOf` entries (longest-prefix-match wins, matching git's own
/// documented algorithm), never touching `gix::url::parse` — `github_url()` above is a
/// plain string matcher and doesn't need a validated `Url` type anyway.
fn resolve_remote_fetch_url(repo: &gix::Repository) -> Option<String> {
    let config = repo.config_snapshot();
    let raw = config.string("remote.origin.url")?.to_string();

    let mut best: Option<(usize, String)> = None;
    if let Some(sections) = config.sections_by_name("url") {
        for section in sections {
            let Some(base) = section.header().subsection_name() else { continue };
            let base = base.to_string();
            for prefix in section.values("insteadOf") {
                let prefix = prefix.to_string();
                if raw.starts_with(prefix.as_str())
                    && best.as_ref().is_none_or(|(len, _)| prefix.len() > *len)
                {
                    best = Some((prefix.len(), base.clone()));
                }
            }
        }
    }

    Some(match best {
        Some((len, base)) => format!("{base}{}", &raw[len..]),
        None => raw,
    })
}

/// `repo.status()`'s entries, deduplicated by path. Used for both the dirty-tree check
/// below and `discovery::is_foreign`'s dirty-tree override (same reasoning applies to both:
/// see the doc comment inline below for why dedup is needed).
///
/// A path that's both staged (index-vs-tree) AND further modified in the worktree
/// (worktree-vs-index) — e.g. `git add`ed, then edited again without re-adding — shows up
/// in real `git status --porcelain` as ONE line (`AM path`). gix's status stream instead
/// emits two separate items for that same path, one `TreeIndex::Addition` and one
/// `IndexWorktree::Modification` (confirmed via a throwaway probe against a real repo in
/// this codebase's own dev tree that had exactly two such paths, inflating
/// `uncommitted_files` by exactly 2 versus the real Python/git-CLI implementation).
pub(crate) fn status_entries(repo: &gix::Repository) -> Vec<String> {
    let Ok(platform) = repo.status(gix::progress::Discard) else {
        return Vec::new();
    };
    let Ok(iter) = platform.into_iter(None) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    iter.filter_map(|item| item.ok())
        .map(|item| item.location().to_string())
        .filter(|loc| seen.insert(loc.clone()))
        .collect()
}

/// `pattern` is a regular expression, exactly like `git log --author=<pattern>` (config's
/// default is literally `"Jan.*Krag"`) — not a literal substring. Matched against the
/// `"Name <email>"` combined author line, the same string git's own `--author` matches
/// against, so a pattern spanning the name/email boundary behaves identically to the CLI.
/// An invalid regex degrades to "never matches" (returns `None`) rather than panicking,
/// consistent with invariant #6's "never an exception" rule extended to this in-process path.
///
/// Also used directly by `discovery::is_foreign`'s authorship check — same query, same
/// semantics, both need "does any commit within the since-horizon match this author".
pub(crate) fn author_since_revwalk(repo: &gix::Repository, pattern: &str, since: &str) -> Option<DateTime<Utc>> {
    let re = regex::Regex::new(pattern).ok()?;
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
        if re.is_match(&author_str) {
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
        let tmp = std::env::temp_dir()
            .join(format!("swab_git_test_{}", std::process::id()))
            .join(name);
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
    fn empty_repo_branch_is_none_matching_real_git_cli_failure() {
        let tmp = make_tmp_dir("empty_repo_branch");
        git_init(&tmp);

        let state = scan(&tmp, &[], "3 years");
        assert_eq!(
            state.branch, None,
            "unborn HEAD: real `git rev-parse --abbrev-ref HEAD` fails, branch must stay None"
        );
    }

    #[test]
    fn author_pattern_is_regex_not_literal_substring() {
        let tmp = make_tmp_dir("author_regex");
        git_init(&tmp);

        let env_author = &[
            ("GIT_AUTHOR_DATE", AUTHOR_DATE),
            ("GIT_COMMITTER_DATE", COMMITTER_DATE),
            ("GIT_AUTHOR_NAME", "Jan Krag"),
            ("GIT_AUTHOR_EMAIL", "jan@example.invalid"),
            ("GIT_COMMITTER_NAME", "Jan Krag"),
            ("GIT_COMMITTER_EMAIL", "jan@example.invalid"),
        ];
        assert!(Command::new("git")
            .args(["-C", tmp.to_str().unwrap(), "commit", "--no-gpg-sign", "-m", "mine", "--allow-empty"])
            .envs(env_author.iter().map(|(k, v)| (k, *v)))
            .spawn().unwrap().wait().unwrap().success());

        let state = scan(&tmp, &["Jan.*Krag".to_string()], "3 years");
        assert!(
            state.mine_last_commit_at.is_some(),
            "\"Jan.*Krag\" must match as a regex against \"Jan Krag <jan@example.invalid>\""
        );
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
    fn https_remote_normalizes_to_github_https() {
        let tmp = make_tmp_dir("https_remote");
        git_init(&tmp);
        git_run(&tmp, &["remote", "add", "origin", "https://github.com/OWNER/REPO.git"]);

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
    fn no_remote_yields_none() {
        let tmp = make_tmp_dir("no_remote");
        git_init(&tmp);
        git_add_and_commit(&tmp, "README.md", "hello");

        let state = scan(&tmp, &[], "3 years");
        assert!(state.is_repo);
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
    fn insteadof_shorthand_remote_resolves_to_real_url() {
        let tmp = make_tmp_dir("insteadof_remote");
        git_init(&tmp);
        git_run(&tmp, &["remote", "add", "origin", "gk:"]);
        assert!(Command::new("git")
            .args([
                "-C", tmp.to_str().unwrap(),
                "config", "--local",
                "url.https://github.com/eficode-academy/git-katas.git.insteadOf", "gk:",
            ])
            .spawn().unwrap().wait().unwrap().success());

        let state = scan(&tmp, &[], "3 years");
        assert_eq!(
            state.github_url,
            Some("https://github.com/eficode-academy/git-katas".to_string()),
            "insteadOf shorthand must resolve to the real GitHub URL, not fail/None"
        );
    }

    #[test]
    fn staged_then_modified_file_counts_as_one_entry() {
        let tmp = make_tmp_dir("staged_then_modified");
        git_init(&tmp);
        git_add_and_commit(&tmp, "README.md", "hello");

        fs::write(tmp.join("new.txt"), "v1").expect("write new.txt");
        assert!(Command::new("git")
            .args(["-C", tmp.to_str().unwrap(), "add", "new.txt"])
            .envs(GIT_ENV.iter().map(|(k, v)| (k, *v)))
            .spawn().unwrap().wait().unwrap().success());
        fs::write(tmp.join("new.txt"), "v2, modified after staging").expect("rewrite new.txt");

        let cli_porcelain = git_run(&tmp, &["status", "--porcelain"]);
        let cli_lines: Vec<&str> = cli_porcelain.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(cli_lines.len(), 1, "real git must report AM as one line: {cli_lines:?}");

        let state = scan(&tmp, &[], "3 years");
        assert_eq!(
            state.uncommitted_files, 1,
            "gix status parity check: staged-then-modified must count as one entry, not two"
        );
    }

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
    fn github_url_non_github_returns_none() {
        assert_eq!(github_url("https://gitlab.com/OWNER/REPO.git"), None);
        assert_eq!(github_url("ssh://git@gitlab.com/OWNER/REPO.git"), None);
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
    fn scan_nonexistent_path_does_not_panic() {
        scan(Path::new("/swab_no_such_dir_xyzzy99"), &[], "3 years");
    }

    /// Commit at an arbitrary real point in time (relative to `Utc::now()` at test-run time,
    /// not a fixed pinned date like `GIT_ENV` -- `daily_commit_counts` buckets relative to
    /// "today", so the fixture's commits must move with it).
    fn git_commit_days_ago(dir: &Path, filename: &str, days_ago: i64) {
        let date = (Utc::now() - chrono::Duration::days(days_ago))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let path = dir.join(filename);
        fs::write(&path, "content").expect("write file");
        let env = [
            ("GIT_AUTHOR_DATE", date.as_str()),
            ("GIT_COMMITTER_DATE", date.as_str()),
            ("GIT_AUTHOR_NAME", "Test Author"),
            ("GIT_AUTHOR_EMAIL", "author@example.com"),
            ("GIT_COMMITTER_NAME", "Test Committer"),
            ("GIT_COMMITTER_EMAIL", "committer@example.com"),
        ];
        assert!(Command::new("git")
            .args(["-C", dir.to_str().unwrap(), "add", filename])
            .envs(env)
            .spawn().expect("git spawn")
            .wait().expect("git add wait")
            .success());
        assert!(Command::new("git")
            .args(["-C", dir.to_str().unwrap(), "commit", "--no-gpg-sign", "-m", "dated commit"])
            .envs(env)
            .spawn().expect("git spawn")
            .wait().expect("git commit wait")
            .success());
    }

    #[test]
    fn daily_commit_counts_buckets_by_day_and_excludes_outside_window() {
        let dir = make_tmp_dir("daily_commits_basic");
        git_init(&dir);
        // Oldest first, matching real chain topology (root -> ... -> HEAD), so the revwalk's
        // descending-commit-time assumption (shared with `author_since_revwalk`) holds.
        git_commit_days_ago(&dir, "old.txt", 20); // outside the 14-day window
        git_commit_days_ago(&dir, "recent.txt", 3); // inside the window
        git_commit_days_ago(&dir, "today.txt", 0); // today

        let repo = gix::open(&dir).expect("repo must open");
        let buckets = daily_commit_counts(&repo, Utc::now());

        assert_eq!(
            buckets.len(), crate::schema::GIT_ACTIVITY_WINDOW_DAYS,
            "buckets must always be exactly GIT_ACTIVITY_WINDOW_DAYS long"
        );
        let window = crate::schema::GIT_ACTIVITY_WINDOW_DAYS;
        assert_eq!(buckets[window - 1], 1, "today's bucket (last) must count the today commit");
        assert_eq!(buckets[window - 1 - 3], 1, "the 3-days-ago commit must land in its own bucket");
        assert_eq!(
            buckets.iter().sum::<u32>(), 2,
            "the 20-days-ago commit is outside the window and must not be counted: {buckets:?}"
        );
    }

    #[test]
    fn daily_commit_counts_repo_with_no_commits_is_all_zero() {
        let dir = make_tmp_dir("daily_commits_empty");
        git_init(&dir);
        let repo = gix::open(&dir).expect("repo must open");
        let buckets = daily_commit_counts(&repo, Utc::now());
        assert_eq!(
            buckets, vec![0u32; crate::schema::GIT_ACTIVITY_WINDOW_DAYS],
            "an unborn-HEAD repo must yield an all-zero window, never panic"
        );
    }

    #[test]
    fn scan_populates_daily_commits_for_a_real_repo() {
        let dir = make_tmp_dir("daily_commits_via_scan");
        git_init(&dir);
        git_commit_days_ago(&dir, "today.txt", 0);

        let state = scan(&dir, &[], "3 years");
        assert!(state.is_repo);
        assert_eq!(
            state.daily_commits.len(), crate::schema::GIT_ACTIVITY_WINDOW_DAYS,
            "scan() must populate daily_commits at the full window length"
        );
        assert_eq!(
            *state.daily_commits.last().unwrap(), 1,
            "today's commit must show up in today's (last) bucket"
        );
    }

    #[test]
    fn not_a_repo_has_empty_daily_commits() {
        let state = GitState::not_a_repo();
        assert!(state.daily_commits.is_empty(), "non-repo must have no daily_commits data");
    }
}
