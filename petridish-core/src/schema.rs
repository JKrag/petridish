//! Wire contract for `projects.json`. Mirrors `src/petridish/schema.py` field-for-field.
//! Field types/names here are the scaffold contract — module R1 fills in the logic
//! (`write_atomic`, `agent_state_for_silence`, bucketing) without changing these shapes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Serde helpers truncating every `DateTime<Utc>` to whole-second precision at the
/// serialization boundary, matching the real wire contract (`petridish.schema.to_utc`/
/// `_iso`, which apply `.replace(microsecond=0)` regardless of where the timestamp
/// originated -- mtime reads, git commit times, etc. can all carry sub-second precision
/// in memory). Without this, a value round-tripped straight from `serde`'s default
/// `DateTime<Utc>` impl (full nanosecond precision) diverges from the Python output on
/// every mtime-derived field (`AgentState.last_event_at`, `Project.last_activity_at`) --
/// caught only once the full aggregator pipeline existed end to end and `diff_check.sh`
/// could run for the first time (see the R9 commit message for how this surfaced).
mod iso_second {
    use chrono::{DateTime, SubsecRound, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(dt: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&dt.trunc_subsecs(0).format("%Y-%m-%dT%H:%M:%SZ").to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime<Utc>, D::Error> {
        let s = String::deserialize(d)?;
        DateTime::parse_from_rfc3339(&s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(serde::de::Error::custom)
    }
}

mod iso_second_opt {
    use chrono::{DateTime, SubsecRound, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        dt: &Option<DateTime<Utc>>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match dt {
            Some(dt) => {
                s.serialize_str(&dt.trunc_subsecs(0).format("%Y-%m-%dT%H:%M:%SZ").to_string())
            }
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<DateTime<Utc>>, D::Error> {
        let opt: Option<String> = Option::deserialize(d)?;
        match opt {
            Some(s) => DateTime::parse_from_rfc3339(&s)
                .map(|dt| Some(dt.with_timezone(&Utc)))
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

/// Silence below this many seconds => `AgentActivity::Working`.
pub const AGENT_WORKING_MAX_S: i64 = 90;
/// Silence below this many seconds (and not `Working`) => `AgentActivity::Recent`.
pub const AGENT_RECENT_MAX_S: i64 = 1800;

/// Samples kept in `Project::agent_activity`, one appended per `swab scan` tick (~60s tick
/// interval -> the full ring spans roughly one hour). Carried forward across ticks by the
/// aggregator itself -- `events.ndjson` cannot supply this history on its own, since it's
/// compacted to a single newest-signal-per-root and truncated every tick (see
/// `swab/src/events.rs`'s module doc). Investigated and confirmed via a real read of this
/// machine's `~/.petridish/events.ndjson` before this field was added.
pub const AGENT_ACTIVITY_WINDOW: usize = 60;

/// Trailing days of daily commit counts kept in `GitState::daily_commits`. Unlike agent
/// activity, this needs no cross-tick carry-forward -- git already retains its own commit
/// history, so it's recomputed fresh from real history every tick.
pub const GIT_ACTIVITY_WINDOW_DAYS: usize = 14;

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
    #[serde(with = "iso_second_opt")]
    pub last_commit_at: Option<DateTime<Utc>>,
    #[serde(with = "iso_second_opt")]
    pub mine_last_commit_at: Option<DateTime<Utc>>,
    pub github_url: Option<String>,
    /// Daily commit counts for the trailing `GIT_ACTIVITY_WINDOW_DAYS` days, oldest first,
    /// today last. Empty when `is_repo` is false. `#[serde(default)]` so a `projects.json`
    /// written before this field existed still deserializes.
    #[serde(default)]
    pub daily_commits: Vec<u32>,
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
            daily_commits: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentState {
    pub state: AgentActivity,
    pub active_agent: Option<String>,
    pub last_event: Option<String>,
    #[serde(with = "iso_second_opt")]
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
    #[serde(default)]
    pub parent_path: Option<String>,
    pub is_foreign: bool,
    pub git: GitState,
    pub agent: AgentState,
    #[serde(with = "iso_second_opt")]
    pub last_activity_at: Option<DateTime<Utc>>,
    pub status_bucket: StatusBucket,
    /// Per-tick agent-event counts for the trailing `AGENT_ACTIVITY_WINDOW` ticks, oldest
    /// first, this tick last. Maintained by `swab scan` carrying it forward from the previous
    /// `projects.json` and appending one new sample per tick -- see `AGENT_ACTIVITY_WINDOW`'s
    /// doc for why this can't be derived from `events.ndjson` alone. `#[serde(default)]` so a
    /// `projects.json` written before this field existed still deserializes.
    #[serde(default)]
    pub agent_activity: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaState {
    #[serde(with = "iso_second_opt")]
    pub measured_at: Option<DateTime<Utc>>,
    pub five_hour_used_pct: Option<u8>,
    #[serde(with = "iso_second_opt")]
    pub five_hour_resets_at: Option<DateTime<Utc>>,
    pub seven_day_used_pct: Option<u8>,
    #[serde(with = "iso_second_opt")]
    pub seven_day_resets_at: Option<DateTime<Utc>>,
    pub context_used_pct: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Radar {
    pub schema_version: u32,
    #[serde(with = "iso_second")]
    pub updated_at: DateTime<Utc>,
    pub scan_duration_ms: u64,
    pub projects: Vec<Project>,
    pub quota: Option<QuotaState>,
}

/// Internal sensor contract — never serialized into `projects.json` (`Radar`/`Project`
/// never hold one). `Serialize` here is for `examples/probe.rs`'s parity-check tooling only.
/// `root` is always a `resolve_root()`-resolved path (invariant #3), never a raw `cwd`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentSignal {
    pub root: String,
    #[serde(with = "iso_second")]
    pub at: DateTime<Utc>,
    pub agent: String,
    pub session_id: Option<String>,
    pub event: Option<String>,
    pub raw_cwd: Option<String>,
}

/// Silence (seconds since last event) -> `AgentActivity`, per `AGENT_WORKING_MAX_S` /
/// `AGENT_RECENT_MAX_S`. Negative silence must clamp to zero, not panic or go negative.
///
/// Mirrors the Python reference (`petridish.schema.agent_state_for_silence`):
/// negative input is treated as zero rather than panicking, a frontend must
/// never crash on a timestamp from the future.
pub fn agent_state_for_silence(silence_seconds: i64) -> AgentActivity {
    let silence = if silence_seconds < 0 { 0 } else { silence_seconds };
    if silence < AGENT_WORKING_MAX_S {
        AgentActivity::Working
    } else if silence < AGENT_RECENT_MAX_S {
        AgentActivity::Recent
    } else {
        AgentActivity::Idle
    }
}

/// Serialize `radar` to `<path>.tmp` (same dir as `path`) then atomically rename onto
/// `path` (invariant #1: daemon is the sole writer, temp-file + rename). On any failure
/// the tmp file must be removed; the parent dir is created if missing.
pub fn write_atomic(path: &Path, radar: &Radar) -> std::io::Result<()> {
    // Same behavior as `petridish.schema.write_atomic`: parent dir first,
    // sibling `.tmp` for the rename to stay on the same filesystem (so
    // `os.replace` / `fs::rename` stays atomic), and cleanup on any failure.
    std::fs::create_dir_all(path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            "radar path has no parent directory",
        )
    })?)?;

    let tmp_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "radar path has a non-UTF8 filename",
            )
        })?;
    let tmp = path.with_file_name(format!("{}.tmp", tmp_name));

    let body = serde_json::to_string(radar).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
    })?;

    std::fs::write(&tmp, &body)?;
    std::fs::rename(&tmp, path)
        .map_err(|rename_err| {
            // Best-effort cleanup: ignore any error from this remove so the
            // original rename failure is what the caller sees.
            let _ = std::fs::remove_file(&tmp);
            rename_err
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::SubsecRound;

    #[test]
    fn agent_state_for_silence_zero_is_working() {
        assert_eq!(agent_state_for_silence(0), AgentActivity::Working);
    }

    #[test]
    fn agent_state_for_silence_just_under_working_is_working() {
        assert_eq!(agent_state_for_silence(89), AgentActivity::Working);
    }

    #[test]
    fn agent_state_for_silence_working_upper_bound_exclusive() {
        // AGENT_WORKING_MAX_S (90) is *not* Working — boundary is exclusive.
        assert_eq!(agent_state_for_silence(90), AgentActivity::Recent);
    }

    #[test]
    fn agent_state_for_silence_just_under_recent_is_recent() {
        assert_eq!(agent_state_for_silence(1799), AgentActivity::Recent);
    }

    #[test]
    fn agent_state_for_silence_recent_upper_bound_is_idle() {
        // AGENT_RECENT_MAX_S (1800) is *not* Recent — falls through to Idle.
        assert_eq!(agent_state_for_silence(1800), AgentActivity::Idle);
    }

    #[test]
    fn agent_state_for_silence_negative_clamps_to_zero() {
        // Negative input must clamp to 0 -> Working, never panic.
        assert_eq!(agent_state_for_silence(-50), AgentActivity::Working);
    }

    #[test]
    fn write_atomic_creates_missing_parent_dir_and_no_tmp_left_behind() {
        let tmp = std::env::temp_dir();
        let dir = tmp.join(format!("swab_write_atomic_test_dir_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir); // start clean

        let path = dir.join("projects.json");
        assert!(!path.exists());
        assert!(!path.parent().unwrap().exists());

        let radar = Radar {
            schema_version: 1,
            updated_at: chrono::Utc::now().trunc_subsecs(0),
            scan_duration_ms: 0,
            projects: vec![],
            quota: None,
        };
        write_atomic(&path, &radar).expect("write_atomic should succeed");

        assert!(path.exists(), "target file must exist after write_atomic");
        // No stray .tmp sibling should be left behind.
        assert!(
            !path.with_file_name("projects.json.tmp").exists(),
            "stray .tmp file must not be left behind"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_overwrite_second_call_differs() {
        let tmp = std::env::temp_dir();
        let dir = tmp.join(format!("swab_write_atomic_overwrite_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let path = dir.join("projects.json");
        let radar_a = Radar {
            schema_version: 1,
            updated_at: chrono::Utc::now().trunc_subsecs(0),
            scan_duration_ms: 0,
            projects: vec![],
            quota: None,
        };
        write_atomic(&path, &radar_a).expect("first write should succeed");

        // Second call: use a different `scan_duration_ms` so the contents differ.
        let mut radar_b = radar_a.clone();
        radar_b.scan_duration_ms = 9999;
        write_atomic(&path, &radar_b).expect("second write should succeed");

        // Read back the file on disk.
        let back = std::fs::read_to_string(&path).expect("file must be readable");

        // File contents must match radar_b, not radar_a.
        let file_json: serde_json::Value =
            serde_json::from_str(&back).expect("file is valid JSON");

        let expected_b: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&radar_b).unwrap()).unwrap();
        assert_eq!(file_json, expected_b);
    }

    #[test]
    fn write_atomic_round_trip_minimal_radar() {
        let radar = Radar {
            schema_version: 1,
            updated_at: chrono::Utc::now().trunc_subsecs(0),
            scan_duration_ms: 0,
            projects: vec![],
            quota: None,
        };
        let s = serde_json::to_string(&radar).expect("serialize minimal Radar");
        let decoded: Radar = serde_json::from_str(&s).expect("round-trip minimal Radar");
        assert_eq!(radar, decoded);
    }
}
