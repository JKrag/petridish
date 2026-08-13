//! `events.ndjson` — the `swab-hook`/`swab-hook-rs` fast path. Mirrors `src/petridish/events.py`.
//!
//! Single-writer invariant (`CLAUDE.md` #1): the hook only ever does one `O_APPEND` write of
//! a single sub-4KB line — it must NEVER open/write `projects.json`. Only the scan-side
//! `read_and_compact` (called once per tick, from `scan.rs`) may resolve+consume this file.

use crate::config::Config;
use crate::schema::AgentSignal;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `$PETRIDISH_EVENTS_PATH` env var override (tests), else `$HOME/.petridish/events.ndjson`.
pub fn events_path() -> PathBuf {
    todo!("R5: env var override else ~/.petridish/events.ndjson")
}

/// One raw hook event as read from stdin JSON. `cwd` is required — the caller must drop
/// the event entirely if absent. `session_id` may come from either `session_id` or
/// `sessionId` (tolerant of both casings); `event` comes from `hook_event_name` and may be
/// absent. This struct is pre-resolution — `resolve_root()` happens only on the read side.
pub struct RawHookEvent {
    pub cwd: String,
    pub session_id: Option<String>,
    pub event: Option<String>,
}

/// Appends one JSON line (`{"cwd", "session_id", "event", "at"}`, `at` stamped here via
/// `Utc::now()` at second precision with a `Z` suffix — NOT taken from the caller) to
/// `events_path()` via a single `O_APPEND` write. Must never panic — any failure (bad path,
/// io error) is swallowed by the caller (`swab-hook-rs`'s `main`), never propagated as a
/// process failure, matching hook.py's bare `except BaseException: return 0`.
pub fn append_event(_path: &Path, _event: &RawHookEvent) -> std::io::Result<()> {
    todo!("R5: single O_APPEND write of one JSON line, at=Utc::now() stamped here")
}

/// Reads `path` line by line, skipping blank/malformed lines silently (invariant #4:
/// truncated trailing JSONL is normal). Stops consuming further lines once cumulative bytes
/// read exceeds `max_bytes` (soft cap, default 5_242_880 — defensive against a daemon-down
/// backlog). For each valid line, resolves `cwd` via `discovery::resolve_root` and folds into
/// one `AgentSignal` per resolved root (newest `at` wins), with `agent` hardcoded to
/// `"claude-code"`. **Truncates the file to empty after reading** — events are consumed
/// exactly once. Missing/unreadable file => empty map, never an error.
pub fn read_and_compact(
    _path: &Path,
    _config: &Config,
    _max_bytes: u64,
) -> HashMap<String, AgentSignal> {
    todo!("R5: parse+fold+resolve_root, truncate file after reading, never raise")
}
