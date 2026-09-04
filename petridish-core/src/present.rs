//! Presentation helpers for converting schema types into strings for the CLI and TUI.
//! These functions map `StatusBucket`, `AgentActivity`, etc. to their display names
//! used in table rendering and filtering.
//!
//! Split out of `swab/src/cli.rs` (slice S3) so the `petri` TUI (`petri/`) can share
//! these formatting functions without depending on `swab` (the writer). `petridish-core`
//! depends on none of the writers, so this crate cannot reach them.

use crate::schema::{waiting_latch_live, AgentActivity, AgentState, GitState, StatusBucket};
use chrono::{DateTime, Utc};

/// `StatusBucket` -> lowercase snake_case string (`"active"`, `"in_flight"`, ...).
///
/// Used by the CLI's `--bucket` filter and table rendering. The schema derives only
/// `Serialize`/`Deserialize` (no `AsRef<str>`), so this free function keeps the
/// schema.rs module clean while the CLI still gets its expected display names.
pub fn status_bucket_str(b: &StatusBucket) -> &'static str {
    match b {
        StatusBucket::Active => "active",
        StatusBucket::InFlight => "in_flight",
        StatusBucket::Stale => "stale",
        StatusBucket::Cold => "cold",
    }
}

/// `AgentActivity` -> lowercase string (`"working"`, `"recent"`, `"idle"`).
///
/// Mirrors the Python CLI's `agent_activity_str` — used in table "agent" column.
pub fn agent_activity_str(a: &AgentActivity) -> &'static str {
    match a {
        AgentActivity::Working => "working",
        AgentActivity::Recent => "recent",
        AgentActivity::Idle => "idle",
    }
}

/// Format the agent column for a project: `"{name} ({state})"` if an agent is active,
/// otherwise just the activity state string.
///
/// A live `waiting_since` latch (`MECH-5`) displaces the silence-derived state entirely
/// rather than appending to it: `"claude-code (waiting)"`, never
/// `"claude-code (idle, waiting)"`. The three silence states are all *inferences* from the
/// absence of events, and "waiting" is an observation that the inference is wrong — showing
/// both would be showing the reader a fact and its own contradiction side by side.
///
/// Takes `now` because the latch expires (`waiting_latch_live`); a caller rendering a stale
/// `projects.json` must not be shown a latch the scanner would already have released.
pub fn agent_label_at(agent: &AgentState, now: DateTime<Utc>) -> String {
    let state = if waiting_latch_live(agent.waiting_since, now) {
        "waiting"
    } else {
        agent_activity_str(&agent.state)
    };
    match &agent.active_agent {
        Some(a) => format!("{a} ({state})"),
        None => state.to_string(),
    }
}

/// `agent_label_at` against the current wall clock — the form every live frontend wants.
pub fn agent_label(agent: &AgentState) -> String {
    agent_label_at(agent, Utc::now())
}

/// Dirty marker for the "dirty" column: `"*"` when repo + dirty, else `" "`.
///
/// Mirrors `cli.rs`'s `_print_table` logic — the marker is part of the table output,
/// not the schema.
pub fn dirty_marker(git: &GitState) -> &'static str {
    if git.is_repo && git.is_dirty { "*" } else { " " }
}

/// Extract the basename of a parent directory path. Returns the last path component,
/// or the original string if no `/` is present (e.g. a bare name).
///
/// Used by `name_cell` to render worktree parent names in the "name" column.
pub fn worktree_parent_name(parent_path: &str) -> String {
    std::path::Path::new(parent_path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| parent_path.to_string())
}

/// Format the project name cell for the "name" column.
///
/// If `parent_path` is set (worktree), returns `"{name} (in {basename})"`.
/// Otherwise, returns just the name.
pub fn name_cell(name: &str, parent_path: Option<&str>) -> String {
    match parent_path {
        Some(parent) => format!("{} (in {})", name, worktree_parent_name(parent)),
        None => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{AgentActivity, AgentState, GitState};

    #[test]
    fn status_bucket_str_all_variants() {
        assert_eq!(status_bucket_str(&StatusBucket::Active), "active");
        assert_eq!(status_bucket_str(&StatusBucket::InFlight), "in_flight");
        assert_eq!(status_bucket_str(&StatusBucket::Stale), "stale");
        assert_eq!(status_bucket_str(&StatusBucket::Cold), "cold");
    }

    #[test]
    fn agent_activity_str_all_variants() {
        assert_eq!(agent_activity_str(&AgentActivity::Working), "working");
        assert_eq!(agent_activity_str(&AgentActivity::Recent), "recent");
        assert_eq!(agent_activity_str(&AgentActivity::Idle), "idle");
    }

    #[test]
    fn agent_label_with_active_agent() {
        let agent = AgentState {
            state: AgentActivity::Working,
            active_agent: Some("claude".to_string()),
            last_event: None,
            last_event_at: None,
            session_id: None,
            waiting_since: None,
        };
        assert_eq!(agent_label(&agent), "claude (working)");
    }

    #[test]
    fn agent_label_at_waiting_displaces_the_silence_state() {
        // The latch is what the reader needs; the silence-derived `Idle` beside it is the
        // inference the latch contradicts. Only one of them belongs in a one-line label.
        let now = Utc::now();
        let agent = AgentState {
            state: AgentActivity::Idle,
            active_agent: Some("claude-code".to_string()),
            waiting_since: Some(now - chrono::Duration::minutes(5)),
            ..AgentState::idle_unknown()
        };
        assert_eq!(agent_label_at(&agent, now), "claude-code (waiting)");
    }

    #[test]
    fn agent_label_at_expired_latch_falls_back_to_the_silence_state() {
        // A frontend reading a `projects.json` the daemon stopped updating must not be shown
        // a latch the scanner would already have released.
        let now = Utc::now();
        let agent = AgentState {
            state: AgentActivity::Idle,
            active_agent: Some("claude-code".to_string()),
            waiting_since: Some(now - chrono::Duration::seconds(crate::schema::WAITING_MAX_LATCH_S + 1)),
            ..AgentState::idle_unknown()
        };
        assert_eq!(agent_label_at(&agent, now), "claude-code (idle)");
    }

    #[test]
    fn agent_label_at_waiting_without_an_agent_name() {
        let now = Utc::now();
        let agent = AgentState {
            waiting_since: Some(now),
            ..AgentState::idle_unknown()
        };
        assert_eq!(agent_label_at(&agent, now), "waiting");
    }

    #[test]
    fn agent_label_without_active_agent() {
        let agent = AgentState::idle_unknown();
        assert_eq!(agent_label(&agent), "idle");
    }

    #[test]
    fn dirty_marker_dirty_repo() {
        let git = GitState {
            is_repo: true,
            branch: Some("main".to_string()),
            is_dirty: true,
            uncommitted_files: 1,
            last_commit_at: None,
            mine_last_commit_at: None,
            github_url: None,
            daily_commits: Vec::new(),
        };
        assert_eq!(dirty_marker(&git), "*");
    }

    #[test]
    fn dirty_marker_clean_repo() {
        let git = GitState::not_a_repo();
        assert_eq!(dirty_marker(&git), " ");
    }

    #[test]
    fn dirty_marker_repo_not_dirty() {
        let git = GitState {
            is_repo: true,
            branch: Some("main".to_string()),
            is_dirty: false,
            uncommitted_files: 0,
            last_commit_at: None,
            mine_last_commit_at: None,
            github_url: None,
            daily_commits: Vec::new(),
        };
        assert_eq!(dirty_marker(&git), " ");
    }

    #[test]
    fn worktree_parent_name_basic() {
        assert_eq!(worktree_parent_name("/Users/jan/repos/catshow-searcher"), "catshow-searcher");
        assert_eq!(worktree_parent_name("/a/b/c"), "c");
    }

    #[test]
    fn worktree_parent_name_no_slash() {
        // Edge case: a path with no `/` falls back to the original string.
        assert_eq!(worktree_parent_name("standalone"), "standalone");
    }

    #[test]
    fn name_cell_no_worktree() {
        assert_eq!(name_cell("my-project", None), "my-project");
    }

    #[test]
    fn name_cell_with_worktree() {
        assert_eq!(
            name_cell("worktree-proj", Some("/Users/jan/repos/catshow-searcher")),
            "worktree-proj (in catshow-searcher)"
        );
    }

    #[test]
    fn name_cell_nested_worktree_parent() {
        assert_eq!(
            name_cell("sub-proj", Some("/deep/nested/path/to/parent")),
            "sub-proj (in parent)"
        );
    }
}
