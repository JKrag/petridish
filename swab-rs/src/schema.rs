//! Wire contract for `projects.json`. Mirrors `src/petridish/schema.py` field-for-field.
//! Field types/names here are the scaffold contract — module R1 fills in the logic
//! (`write_atomic`, `agent_state_for_silence`, bucketing) without changing these shapes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Silence below this many seconds => `AgentActivity::Working`.
pub const AGENT_WORKING_MAX_S: i64 = 90;
/// Silence below this many seconds (and not `Working`) => `AgentActivity::Recent`.
pub const AGENT_RECENT_MAX_S: i64 = 1800;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActivity {
    Working,
    Recent,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusBucket {
    Active,
    InFlight,
    Stale,
    Cold,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitState {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub is_dirty: bool,
    pub uncommitted_files: u32,
    pub last_commit_at: Option<DateTime<Utc>>,
    pub mine_last_commit_at: Option<DateTime<Utc>>,
    pub github_url: Option<String>,
}

impl GitState {
    /// `GitState { is_repo: false, .. }` — the required "never an exception" fallback
    /// for a failed/timed-out git call (invariant #6).
    pub fn not_a_repo() -> Self {
        GitState {
            is_repo: false,
            branch: None,
            is_dirty: false,
            uncommitted_files: 0,
            last_commit_at: None,
            mine_last_commit_at: None,
            github_url: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentState {
    pub state: AgentActivity,
    pub active_agent: Option<String>,
    pub last_event: Option<String>,
    pub last_event_at: Option<DateTime<Utc>>,
    pub session_id: Option<String>,
}

impl AgentState {
    /// The degrade-never-abort default (invariant #5) — no signal for this project.
    pub fn idle_unknown() -> Self {
        AgentState {
            state: AgentActivity::Idle,
            active_agent: None,
            last_event: None,
            last_event_at: None,
            session_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub category: String,
    pub is_foreign: bool,
    pub git: GitState,
    pub agent: AgentState,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub status_bucket: StatusBucket,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaState {
    pub measured_at: Option<DateTime<Utc>>,
    pub five_hour_used_pct: Option<u8>,
    pub five_hour_resets_at: Option<DateTime<Utc>>,
    pub seven_day_used_pct: Option<u8>,
    pub seven_day_resets_at: Option<DateTime<Utc>>,
    pub context_used_pct: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Radar {
    pub schema_version: u32,
    pub updated_at: DateTime<Utc>,
    pub scan_duration_ms: u64,
    pub projects: Vec<Project>,
    pub quota: Option<QuotaState>,
}

/// Internal sensor contract — never serialized into `projects.json`.
/// `root` is always a `resolve_root()`-resolved path (invariant #3), never a raw `cwd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSignal {
    pub root: String,
    pub at: DateTime<Utc>,
    pub agent: String,
    pub session_id: Option<String>,
    pub event: Option<String>,
    pub raw_cwd: Option<String>,
}

/// Silence (seconds since `at`) -> `AgentActivity`, per `AGENT_WORKING_MAX_S` /
/// `AGENT_RECENT_MAX_S`. Negative silence must clamp to zero, not panic or go negative.
pub fn agent_state_for_silence(_silence_seconds: i64) -> AgentActivity {
    todo!("R1: threshold silence against AGENT_WORKING_MAX_S / AGENT_RECENT_MAX_S, clamp negative to 0")
}

/// Serialize `radar` to `<path>.tmp` (same dir as `path`) then atomically rename onto
/// `path` (invariant #1: daemon is the sole writer, temp-file + rename). On any failure
/// the tmp file must be removed; the parent dir is created if missing.
pub fn write_atomic(_path: &Path, _radar: &Radar) -> std::io::Result<()> {
    todo!("R1: create parent dir if missing, write to sibling .tmp, fs::rename onto path, clean up .tmp on failure")
}
