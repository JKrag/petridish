//! Claude Code transcript sensor. Mirrors `src/petridish/sensors/claude.py`.
//! The primary implementation of invariants #2–#4 (`CLAUDE.md`) — read these verbatim
//! findings from `IMPLEMENTATION_PLAN.md` §0 before writing this module's logic:
//!
//! - F2: slug is not reversibly decodable — `cwd` is read from JSONL content, NEVER the
//!   `~/.claude/projects/<slug>/` dirname.
//! - F9: `cwd` varies within one transcript — take it from the **last** line (in file-scan
//!   order) that carries a `cwd` field, then resolve via `discovery::resolve_root`.
//! - F3 / invariant #3: liveness = file **mtime** recency, not any timestamp inside the
//!   JSON — there is no reliable "working vs idle" state machine from transcript content.
//! - Invariant #4: truncated trailing JSONL lines are normal; skip and fall back, never error.

use crate::config::Config;
use crate::schema::AgentSignal;
use std::collections::HashMap;
use std::path::Path;

/// Bytes to seek back from EOF before scanning forward (tail-read optimization) — the first
/// (likely partial) line after the seek is discarded. Falls back to reading the whole file
/// from the top only if no `cwd` was found in the tail window.
pub const TAIL_BYTES: u64 = 65536;

/// Directories under `~/.claude/projects/` whose transcripts haven't been touched in this
/// many hours are skipped before even opening the file (cold-skip).
pub const DEFAULT_COLD_CUTOFF_HOURS: u64 = 1440;

/// Walks `claude_projects_dir` (`~/.claude/projects/*/*.jsonl`), building one `AgentSignal`
/// per resolved project root. Per transcript file: `session_id` = first line (scan order)
/// with a `sessionId` string field; `cwd` = last line (scan order) with a `cwd` string field
/// (F9), resolved via `discovery::resolve_root`; `at` = the file's mtime (F3), not any JSON
/// timestamp. Cold files (mtime older than `cold_cutoff_hours`) are skipped without opening.
/// Malformed/truncated JSON lines are caught and skipped, never fatal (invariant #4). Two
/// transcripts resolving to the same root fold to one signal, newest file mtime wins. A
/// missing `claude_projects_dir` returns an empty map, never an error (invariant #5).
pub fn scan(
    _claude_projects_dir: &Path,
    _config: &Config,
    _cold_cutoff_hours: u64,
) -> HashMap<String, AgentSignal> {
    todo!("R6: tail-read + full-file fallback, last-cwd/first-session_id scan order, cold-skip, resolve_root fold")
}
