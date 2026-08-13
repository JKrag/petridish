//! Git facts for one path. Mirrors `src/petridish/git.py`.
//!
//! Invariant #6 (`CLAUDE.md`): "`git` calls use `subprocess.run` with `check=False` and a
//! 5s timeout. A git failure is a `GitState(is_repo=False)`, never an exception." In Rust,
//! `std::process::Command::output()` has **no built-in timeout** — this module must wrap
//! every git invocation with the `wait-timeout` crate (already in Cargo.toml) or an
//! equivalent thread+kill, and must never let a git failure/timeout become a panic or `Err`
//! that propagates past this module.

use crate::schema::GitState;
use std::path::Path;
use std::time::Duration;

pub const GIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Runs: `rev-parse --git-dir` (is-a-repo), `rev-parse --abbrev-ref HEAD` (branch),
/// `status --porcelain` (dirty + uncommitted file count), `log -1 --format=%cI` (last
/// commit), `log -1 --format=%cI --author=<pattern> --since=<horizon>` for each of
/// `config.author_patterns` (first match wins, for `mine_last_commit_at`), and
/// `remote get-url origin` normalized to an `https://github.com/...` URL (SSH or HTTPS
/// remote forms both normalize; a non-GitHub remote or no remote => `None`). Any command
/// failing, erroring, or timing out (5s, `GIT_TIMEOUT`) => that field is `None`/default,
/// never a panic — and if `rev-parse --git-dir` itself fails, short-circuit to
/// `GitState::not_a_repo()` without running the rest.
pub fn scan(_path: &Path, _author_patterns: &[String], _author_since: &str) -> GitState {
    todo!("R4: shell out to git with a 5s timeout wrapper (wait-timeout), check=false semantics, never panic")
}
