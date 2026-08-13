//! The aggregator ("the tick"). Mirrors `src/petridish/scan.py::run_scan`.
//!
//! ⚠️ Extra scrutiny module (see plan step 3.8): the Python equivalent shipped a real bug —
//! sensor directories defaulted to `None`/unset and were silently swallowed by a broad error
//! catch, producing a healthy-looking but EMPTY `projects.json` in production, invisible to
//! 126 passing tests because integration tests inject fixture paths and never exercise the
//! production default path. **After implementing this module, run the built binary for real
//! against a real `$HOME` and eyeball the output — do not rely on `cargo test` alone.**
//! Every sensor directory here must have a real, non-empty production default (mirroring
//! `~/.claude/projects`, VS Code's real `workspaceStorage`, `~/.petridish/events.ndjson`),
//! never a silently-`None` parameter that only fixture-injecting tests ever override.

use crate::config::Config;
use crate::schema::Radar;
use std::path::Path;

pub struct ScanPaths {
    pub claude_projects_dir: std::path::PathBuf,
    pub workspace_storage_dir: std::path::PathBuf,
    pub events_path: std::path::PathBuf,
    pub quota_path: std::path::PathBuf,
}

impl ScanPaths {
    /// Composes all four sensor paths relative to `home`. Kept separate from
    /// `production_defaults()` so tests can exercise the exact real path composition against
    /// a tmp dir without mutating the process-wide `HOME` env var (`unsafe` under the 2024
    /// edition, and racy under parallel tests regardless of edition).
    pub fn for_home(home: &Path) -> Self {
        ScanPaths {
            claude_projects_dir: home.join(".claude").join("projects"),
            workspace_storage_dir: home
                .join("Library")
                .join("Application Support")
                .join("Code")
                .join("User")
                .join("workspaceStorage"),
            events_path: crate::events::events_path(),
            quota_path: home.join(".claude").join("last-status.json"),
        }
    }

    /// Real production defaults, reading `$HOME` from the environment — must resolve to the
    /// actual `$HOME`-relative locations, never `None`/empty. This is the exact seam the
    /// Python original's production bug lived in; get it right here (see this module's
    /// doc comment above).
    pub fn production_defaults() -> Self {
        let home = std::env::var("HOME").expect("HOME must be set");
        Self::for_home(Path::new(&home))
    }
}

/// One full tick: `discovery::discover` + `discovery::is_foreign` for project list, `git::scan`
/// per project, all three sensors + `events::read_and_compact` merged into one
/// `root -> AgentSignal` map (invariant #5: any sensor erroring/panicking-internally must
/// degrade to an empty contribution, never abort the tick), newest-`at`-wins across sources.
/// Every discovered path AND every signal root becomes a `Project` (union, not intersection).
/// `last_activity_at` = max(`mine_last_commit_at` preferred over `last_commit_at`, merged
/// signal's `at`). Final sort: `last_activity_at` descending (`None` last), then `name`
/// ascending. Times the whole tick for `scan_duration_ms`. Never panics — see the sensor
/// degrade-never-abort invariant.
pub fn run_scan(_config: &Config, _paths: &ScanPaths) -> Radar {
    todo!("R8: wire discovery+git+sensors+events into one Radar, union+merge+bucket+sort")
}

/// `run_scan` + `schema::write_atomic` to `state_path`. Returns the written `Radar` (for the
/// CLI's "scanned N projects in Mms -> path" message) or an io error from the write step.
pub fn write_scan(
    _config: &Config,
    _paths: &ScanPaths,
    _state_path: &Path,
) -> std::io::Result<Radar> {
    todo!("R8: run_scan then write_atomic, propagate only the write's io::Result")
}
