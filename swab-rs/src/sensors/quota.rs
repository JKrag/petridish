//! Reads Claude Code's own `~/.claude/last-status.json` for quota/rate-limit display.
//! Mirrors `src/petridish/sensors/quota.py`. Account-global (not per-project) — feeds
//! `Radar.quota`, never a `Project`.

use crate::schema::QuotaState;
use std::path::Path;

/// Reads and parses `path`. Returns `None` (never panics/errors) on: missing file, unreadable
/// file, malformed JSON, a non-mapping top-level payload, or out-of-range/boolean percentage
/// fields. Partial payloads degrade field-by-field — a dropped window key or renamed/
/// mis-nested field leaves the rest of the struct intact rather than failing the whole parse.
/// A naive (no-tz) timestamp is assumed UTC. An implausible reset timestamp (more than a
/// 30-day horizon out) is dropped (that field => `None`) rather than trusted.
pub fn read_quota(_path: &Path) -> Option<QuotaState> {
    todo!("R7: tolerant partial-payload parse of last-status.json, 30-day plausibility check on resets_at")
}
