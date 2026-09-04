//! `events.ndjson` — the `swab-hook`/`swab-hook-rs` fast path. Mirrors `src/petridish/events.py`.
//!
//! Single-writer invariant (`CLAUDE.md` #1): the hook only ever does one `O_APPEND` write of
//! a single sub-4KB line — it must NEVER open/write `projects.json`. Only the scan-side
//! `read_and_compact` (called once per tick, from `scan.rs`) may resolve+consume this file.

use crate::config::Config;
use crate::discovery;
use crate::schema::AgentSignal;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// `$PETRIDISH_EVENTS_PATH` env var override (tests), else `$HOME/.petridish/events.ndjson`.
pub fn events_path() -> PathBuf {
    if let Ok(override_path) = std::env::var("PETRIDISH_EVENTS_PATH") {
        return PathBuf::from(override_path);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "~".to_string());
    PathBuf::from(home).join(".petridish").join("events.ndjson")
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

/// Format a `Utc` timestamp at second precision with a trailing `Z`, per the JSONL wire
/// contract (`hook.py` line 42).
fn format_at(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Parse a wire-format ISO-8601 timestamp (trailing `Z` replaced with `+00:00`). The wire
/// format is the one `append_event` always emits, but callers accept either form so a
/// `read_and_compact` run over an older file with an offset-form `at` still parses.
fn parse_at(s: &str) -> Option<DateTime<Utc>> {
    // The wire format `append_event` emits is `%Y-%m-%dT%H:%M:%SZ`. We accept two
    // forms: trailing `Z` (strip + synthesize UTC offset) or RFC3339 with explicit
    // fixed offset (e.g. `+00:00`). In both cases we end up with a fixed-offset
    // datetime at UTC.
    let body = s.strip_suffix('Z').unwrap_or(s);
    let with_offset = if body.contains('+') || body.ends_with("+00:00") {
        body.to_string()
    } else if body.contains('T') && !body.ends_with('+') {
        // T-separated local time without explicit offset — treat as UTC.
        format!("{body}+00:00")
    } else {
        body.to_string()
    };

    // Try with `%z` first (the wire form always has one), then fall through to
    // no-offset parsing — if both fail, the value is malformed.
    DateTime::parse_from_str(&with_offset, "%Y-%m-%dT%H:%M:%S%z")
        .or_else(|_| DateTime::parse_from_str(&with_offset, "%Y-%m-%dT%H:%M:%S"))
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// One `events.ndjson` record, in the exact field order the file is written in.
///
/// Declared as a struct rather than assembled into a `serde_json::Map`, and that
/// is load-bearing rather than stylistic. A `Map` is a `BTreeMap` — its key order
/// is alphabetical *by accident of the default feature set*, and enabling
/// serde_json's `preserve_order` feature anywhere in the workspace silently flips
/// it to insertion order. Cargo unifies features across the whole graph, so a
/// sibling crate turning that feature on for its own reasons would rewrite this
/// file's wire format as a side effect. `events.ndjson` is shared with three
/// other hook consumers, so serde's declaration-order guarantee is what actually
/// pins it. Field order here is therefore the file format: do not reorder.
#[derive(serde::Serialize)]
struct EventLine<'a> {
    at: String,
    cwd: &'a str,
    event: Option<&'a str>,
    session_id: Option<&'a str>,
}

/// Builds one JSON line, then formats with `serde_json::to_string`.
/// Returns `None` when the line is so malformed we'd rather silently skip it — callers
/// don't propagate errors, matching hook.py's "never raise" invariant.
fn build_json_line<'a>(event: &'a RawHookEvent, at: DateTime<Utc>) -> Option<EventLine<'a>> {
    // `cwd` must never be blank — mirroring hook.py's `if not cwd: return 0`.
    if event.cwd.is_empty() {
        return None;
    }
    Some(EventLine {
        at: format_at(at),
        cwd: &event.cwd,
        event: event.event.as_deref(),
        session_id: event.session_id.as_deref(),
    })
}

/// Writes a single JSON line to the events file with `OpenOptions::create(true).append(true)`,
/// using one `write_all` of the entire serialized line plus `\n`. This is what keeps
/// concurrent appends from multiple `swab-hook-rs` processes safe without locking:
/// `O_APPEND` ensures each write is atomic, and the single `write_all` makes sure the line
/// isn't interleaved with a sibling's partial write.
fn write_single_line(path: &Path, event: &RawHookEvent, at: DateTime<Utc>) -> std::io::Result<()> {
    let Some(value) = build_json_line(event, at) else {
        // `cwd` empty — silently drop, no error. Mirrors hook.py's "no-op on missing cwd".
        return Ok(());
    };
    let body = serde_json::to_string(&value).map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut line_buf = body.into_bytes();
    line_buf.push(b'\n');

    // Mirrors `os.open(events_path, os.O_WRONLY | os.O_CREAT | os.O_APPEND)`.
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    // One `write_all` for the entire line — split into multiple writes would defeat the
    // O_APPEND atomicity guarantee (the kernel interleaves at the syscall boundary).
    file.write_all(&line_buf)?;
    Ok(())
}

pub fn append_event(path: &Path, event: &RawHookEvent) -> std::io::Result<()> {
    write_single_line(path, event, Utc::now())
}

/// Hook event names that mean "this agent is blocked on a human" (`MECH-5`). Claude Code
/// fires `Notification` when it needs your attention (a permission prompt, or a turn that
/// has been idle waiting for input) and `PermissionRequest` when a tool call is specifically
/// held for approval. Verified against a real `~/.claude/settings.json` on Claude Code
/// 2.1.236 — both are live event names there, registered by three other consumers.
pub const WAITING_SET_EVENTS: [&str; 2] = ["Notification", "PermissionRequest"];

/// Hook event names that mean "the human answered, carry on" (`MECH-5`). Both are already
/// registered (`installer.py`'s `HOOK_EVENTS`), which is the whole reason the clearing half
/// of this feature needs no new hook invocations: `PreToolUse` fires as the agent resumes
/// tool work, `Stop` as the turn ends.
pub const WAITING_CLEAR_EVENTS: [&str; 2] = ["PreToolUse", "Stop"];

/// What this pass observed about one root's `MECH-5` waiting state: `true` = a set event was
/// the last relevant thing seen, `false` = a clear event was. A root absent from the map saw
/// neither, which is *not* the same as `false` — it means "no news", and the scanner must
/// carry the previous tick's latch forward rather than releasing it.
pub type WaitingDeltas = HashMap<String, bool>;

/// Reads and folds an events ndjson file into one `AgentSignal` per resolved root, then
/// truncates the file. See module-level doc for full contract — summary below.
///
/// `path` may be missing or unreadable: we return `HashMap::new()` in either case and
/// never propagate an error (invariant #5 — sensors degrade, never abort).
///
/// `max_bytes` is a soft cap — once cumulative bytes read exceed it, further lines are
/// dropped on this pass (the file is still truncated at the end, so those events don't
/// come back).
///
/// Returns `(signals, counts, waiting)`: `signals` is the existing newest-`at`-wins fold, one entry
/// per resolved root. `counts` is a second, independent tally -- how many raw valid lines
/// resolved to each root *this pass* -- kept alongside the fold rather than replacing it,
/// since bucketing/agent-state derivation still wants "the latest signal", while the
/// agent-activity sparkline (`Project::agent_activity`) wants "how many events fired this
/// tick", which the fold alone discards. `waiting` is the third such tally (`MECH-5`) — see
/// `WaitingDeltas`.
pub fn read_and_compact(
    path: &Path,
    config: &Config,
    max_bytes: u64,
) -> (
    HashMap<String, AgentSignal>,
    HashMap<String, u32>,
    WaitingDeltas,
) {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return (HashMap::new(), HashMap::new(), HashMap::new()),
    };

    let mut signals: HashMap<String, AgentSignal> = HashMap::new();
    let mut counts: HashMap<String, u32> = HashMap::new();
    let mut waiting: WaitingDeltas = HashMap::new();
    let mut bytes_seen: u64 = 0;

    // Split on either line-ending character in one pass. The previous form --
    // `content.split('\n').chain(content.split('\r'))` -- silently double-processed every
    // line whenever the file had zero `\r` bytes (always true here; `append_event` only ever
    // writes `\n`): `content.split('\r')` on a string with no `\r` yields the WHOLE file as
    // one element, which for a single-line file happens to still parse as valid JSON,
    // double-counting that line. The signal fold (newest-`at`-wins overwrite) silently
    // masked this; it surfaced once a genuine per-line counter was added alongside the fold.
    for raw_line in content.split(['\n', '\r']) {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        let line_bytes = line.len() as u64;
        if bytes_seen > 0 && bytes_seen + line_bytes > max_bytes {
            // Soft cap reached — drop the rest per Python original.
            break;
        }

        bytes_seen += line_bytes;

        let record: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // malformed / truncated line — skip silently.
        };
        let obj = match record.as_object() {
            Some(o) => o,
            None => continue,
        };

        // `cwd` required — drop the line if absent (matches Python's KeyError path).
        let cwd = match obj.get("cwd").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };

        let session_id = obj
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(String::from);
        let event = obj.get("event").and_then(|v| v.as_str()).map(String::from);
        let at_str = match obj.get("at").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue, // `at` is stored by append_event — if missing, malformed.
        };

        let at = match parse_at(&at_str) {
            Some(dt) => dt,
            None => continue, // Unparseable timestamp — malformed line.
        };

        let cwd_path = PathBuf::from(cwd.clone());
        let resolved = discovery::resolve_root(&cwd_path, config);

        let signal = AgentSignal {
            root: resolved.to_string_lossy().to_string(),
            at,
            agent: "claude-code".to_string(),
            session_id,
            event,
            raw_cwd: Some(cwd),
        };

        let key = signal.root.clone();
        *counts.entry(key.clone()).or_insert(0) += 1;

        // `MECH-5`: last-relevant-line-wins, in **file order**, deliberately not by
        // comparing `at`. `format_at` writes whole seconds, so a `Notification` and the
        // `PreToolUse` that answers it can and do land in the same second; `O_APPEND` order
        // is then the only truthful chronology we have. (Note also that the signal fold
        // above uses a keep-the-earlier tie rule — the opposite convention, correct there
        // and wrong here, which is why this does not reuse it.)
        match signal.event.as_deref() {
            Some(e) if WAITING_SET_EVENTS.contains(&e) => {
                waiting.insert(key.clone(), true);
            }
            Some(e) if WAITING_CLEAR_EVENTS.contains(&e) => {
                waiting.insert(key.clone(), false);
            }
            // Any other event name says nothing about waiting — leave whatever the previous
            // line decided in place, rather than treating "not a clear event" as a clear.
            _ => {}
        }
        match signals.get(&key) {
            Some(existing) if existing.at >= at => {
                // Keep the newer one — the Python `if at > existing.at` check is
                // strict "newer wins"; tie goes to the earlier-stored entry.
            }
            _ => {
                signals.insert(key, signal);
            }
        }
    }

    // Truncate — events consumed exactly once. Even if we stopped early on the
    // `max_bytes` cap, everything past the cap is dropped. Per Python's
    // "write() with empty body" after reading, which simply opens `w` and closes it.
    let _ = std::fs::write(path, "");

    (signals, counts, waiting)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Test helper: write `contents` to a unique temp file and return the path.
    /// Each call gets its own subdir, so concurrent tests don't clobber one
    /// another's state via the shared `~/.tmp/swab_test_events/*` layout.
    fn with_tmp(name: &str, contents: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static CTR: AtomicU64 = AtomicU64::new(0);
        let id = CTR.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("swab_test_events_{id}_{}", std::process::id()));
        // Another thread may have created it first; that's fine.
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }

    // Move the static outside the fn body — Rust scoping requires it live in
    // module scope, not inside a function.

    fn test_config(roots: Vec<PathBuf>) -> Config {
        Config {
            roots,
            ..Config::default()
        }
    }

    /// Byte-exact lock on the `events.ndjson` wire format.
    ///
    /// `events.ndjson` is read by three hook consumers besides ours, so its key
    /// order and null-handling are a contract, not an implementation detail.
    /// Before `EventLine` existed the line was built from a `serde_json::Map`,
    /// whose ordering is alphabetical only because serde_json's `preserve_order`
    /// feature is off — and Cargo unifies features across the workspace, so any
    /// sibling crate enabling it (the installer wants it, to avoid reordering a
    /// user's `~/.claude/settings.json`) would have silently rewritten this file
    /// from `{at, cwd, event, session_id}` to insertion order.
    ///
    /// These two strings are the bytes the shipped `swab-hook` binary produced
    /// before that change. If this test fails, the wire format moved.
    #[test]
    fn json_line_bytes_are_the_frozen_wire_format() {
        let at = DateTime::parse_from_rfc3339("2026-09-04T15:45:17Z")
            .unwrap()
            .with_timezone(&Utc);

        let full = RawHookEvent {
            cwd: "/tmp/demo".to_string(),
            session_id: Some("abc123".to_string()),
            event: Some("PreToolUse".to_string()),
        };
        assert_eq!(
            serde_json::to_string(&build_json_line(&full, at).unwrap()).unwrap(),
            r#"{"at":"2026-09-04T15:45:17Z","cwd":"/tmp/demo","event":"PreToolUse","session_id":"abc123"}"#,
        );

        // Absent optionals serialize as explicit `null`, not as omitted keys —
        // consumers index by key and a missing key is not the same as a null one.
        let bare = RawHookEvent {
            cwd: "/tmp/demo".to_string(),
            session_id: None,
            event: None,
        };
        assert_eq!(
            serde_json::to_string(&build_json_line(&bare, at).unwrap()).unwrap(),
            r#"{"at":"2026-09-04T15:45:17Z","cwd":"/tmp/demo","event":null,"session_id":null}"#,
        );
    }

    /// Test 1: append_event writes exactly one line, valid JSON, with `at` matching
    /// (approximately) Utc::now() at write time.
    #[test]
    fn append_event_writes_one_valid_line_with_fresh_at() {
        let path = with_tmp("append_event_one_line.ndjson", "");

        // Move write boundary far enough away from current time to be confident
        // Utc::now() at write time lies between `start` and `end`. We don't assert
        // the exact value (system clock may advance between calls), just that it's
        // a recent ISO-8601 timestamp. The `at` field is formatted at second
        // precision, so we floor/ceiling the boundary with seconds too.
        let start = Utc::now();
        let event = RawHookEvent {
            cwd: "/tmp/test_swab_events_1".to_string(),
            session_id: Some("s-abc".to_string()),
            event: Some("tool_use".to_string()),
        };
        append_event(&path, &event).expect("append must succeed");
        let end = Utc::now();

        let content = std::fs::read_to_string(&path).expect("file must be readable");
        let lines: Vec<&str> = content.trim_end().split('\n').collect();
        assert_eq!(lines.len(), 1, "exactly one line, got {:?}", lines);

        let record: Value = serde_json::from_str(lines[0]).expect("line must be valid JSON");
        assert_eq!(record["cwd"], "/tmp/test_swab_events_1");
        assert_eq!(record["session_id"], "s-abc");
        assert_eq!(record["event"], "tool_use");

        let at_str = record["at"].as_str().unwrap();
        // Strip trailing `Z` and round-trip to parse, then check monotonicity.
        assert!(at_str.ends_with('Z'), "at must end with Z: {at_str:?}");
        let at_dt = parse_at(at_str).expect("stored at must be parseable");
        // `at_dt` has second precision — floor/ceiling both boundaries to seconds
        // so a `Utc::now()` that fires mid-second doesn't fail the comparison.
        let start_floor = start.timestamp() as u64;
        let end_ceiling = (end.timestamp() + 1) as u64;
        let at_secs = at_dt.timestamp() as u64;
        assert!(
            at_secs >= start_floor && at_secs <= end_ceiling,
            "at {at_dt} not in [{start_floor}, {end_ceiling}]"
        );
    }

    /// Test 2: Two sequential append_event calls -> file has exactly two lines, both
    /// parseable.
    #[test]
    fn append_event_two_calls_yields_two_lines() {
        let path = with_tmp("append_event_two.ndjson", "");

        for i in 0..2 {
            let event = RawHookEvent {
                cwd: format!("/tmp/event_{i}"),
                session_id: None,
                event: Some("goose".to_string()),
            };
            append_event(&path, &event).expect("append must succeed");
        }

        let content = std::fs::read_to_string(&path).expect("file must be readable");
        let non_empty_lines: Vec<&str> = content
            .trim_end()
            .split('\n')
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(
            non_empty_lines.len(),
            2,
            "two calls -> two lines, got: {:?}",
            non_empty_lines
        );

        for line in &non_empty_lines {
            serde_json::from_str::<Value>(line).expect("each line must be valid JSON");
        }

        // Line 1 and line 2 must have different `at` (seconds differ) or equal —
        // either is fine, as long as both are parseable ISO-8601 `Z` strings.
        for line in &non_empty_lines {
            let rec: Value = serde_json::from_str(line).unwrap();
            let at = rec["at"].as_str().unwrap();
            assert!(at.ends_with('Z'));
        }
    }

    /// Test 3: read_and_compact on a missing file -> empty map, no panic.
    #[test]
    fn read_and_compact_missing_path_returns_empty() {
        let path = PathBuf::from("/tmp/does_not_exist_swab_test_xyzzy99_100/events.ndjson");
        let cfg = test_config(vec![]);
        let (signals, counts, _waiting) = read_and_compact(&path, &cfg, 5_000_000);
        assert!(
            signals.is_empty(),
            "missing file -> empty map, got: {:?}",
            signals
        );
        assert!(
            counts.is_empty(),
            "missing file -> empty counts, got: {:?}",
            counts
        );
    }

    /// Test 4: read_and_compact skips a malformed line (invalid JSON) but keeps a valid
    /// line on either side of it.
    #[test]
    fn read_and_compact_skips_malformed_between_valid() {
        let content = r#"{"cwd":"/tmp/p1","at":"2024-01-01T00:00:01Z"}
not valid json
{"cwd":"/tmp/p2","at":"2024-01-01T00:00:02Z"}
"#;
        let path = with_tmp("read_skips_malformed.ndjson", content);
        let cfg = test_config(vec![PathBuf::from("/tmp")]);
        let (result, _counts, _waiting) = read_and_compact(&path, &cfg, 10_000_000);

        // Valid lines have cwd-as-is (no .git found, so resolve_root returns input).
        // Both entries should be in the map with their respective session_id (None here).
        let keys: Vec<&String> = result.keys().collect();
        assert_eq!(
            keys.len(),
            2,
            "two valid lines -> two entries, got {:?}",
            keys
        );

        // File should be truncated after read.
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.is_empty(), "file must be truncated: {after:?}");
    }

    /// Test 5: read_and_compact folds two lines with the same resolved root -> one signal,
    /// newest `at` wins.
    #[test]
    fn read_and_compact_folds_same_root_newest_at_wins() {
        let content = r#"{"cwd":"/tmp/p1","at":"2024-01-01T00:00:01Z","session_id":"s1"}
{"cwd":"/tmp/p1","at":"2024-01-01T00:00:05Z","session_id":"s2"}
"#;
        let path = with_tmp("read_folds_same_root.ndjson", content);
        let cfg = test_config(vec![PathBuf::from("/tmp")]);
        let (result, counts, _waiting) = read_and_compact(&path, &cfg, 10_000_000);

        assert_eq!(result.len(), 1, "two lines same root -> one entry");
        let entry = result.values().next().unwrap();
        // The newest at (2024-01-01T00:00:05Z) should win; its session_id is "s2".
        // Note `to_rfc3339_opts` uses `Z` for zero-offset UTC.
        assert_eq!(
            entry.at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "2024-01-01T00:00:05Z"
        );
        assert_eq!(entry.session_id.as_deref(), Some("s2"));
        // The fold collapses to one signal, but the count tally still sees both lines.
        assert_eq!(
            counts.values().next().copied(),
            Some(2),
            "two raw lines for the same root -> count of 2, got {:?}",
            counts
        );
    }

    /// Test 6: read_and_compact TRUNCATES the file after reading — assert the file is
    /// empty (0 bytes) afterward, and a second call returns an empty map.
    #[test]
    fn read_and_compact_truncates_file_after() {
        let content = r#"{"cwd":"/tmp/xyz","at":"2024-01-01T00:00:01Z","session_id":"s1"}
"#;
        let path = with_tmp("read_truncates.ndjson", content);

        let cfg = test_config(vec![PathBuf::from("/tmp")]);
        let (first, first_counts, _waiting) = read_and_compact(&path, &cfg, 10_000_000);
        assert_eq!(first.len(), 1, "first read should pick up one signal");
        assert_eq!(
            first_counts.values().sum::<u32>(),
            1,
            "first read should count one event"
        );

        // File must be empty after truncation.
        let size = std::fs::metadata(&path).expect("meta").len();
        assert_eq!(size, 0, "file size must be 0 after truncation, got {size}");

        let (second, second_counts, _waiting) = read_and_compact(&path, &cfg, 10_000_000);
        assert!(
            second.is_empty(),
            "second read of truncated file must be empty, got {:?}",
            second
        );
        assert!(
            second_counts.is_empty(),
            "second read must see no counts either"
        );
    }

    /// Test 7: read_and_compact on a line missing `cwd` -> that line is dropped
    /// (skipped), doesn't error the whole read.
    #[test]
    fn read_and_compact_skips_line_missing_cwd() {
        let content = r#"{"cwd":"/tmp/a","at":"2024-01-01T00:00:01Z","session_id":"a"}
{"at":"2024-01-01T00:00:02Z","session_id":"no-cwd"}
{"cwd":"/tmp/b","at":"2024-01-01T00:00:03Z","session_id":"b"}
"#;
        let path = with_tmp("read_skips_missing_cwd.ndjson", content);
        let cfg = test_config(vec![PathBuf::from("/tmp")]);
        let (result, _counts, _waiting) = read_and_compact(&path, &cfg, 10_000_000);

        let keys: Vec<&String> = result.keys().collect();
        assert_eq!(keys.len(), 2, "missing cwd line dropped: got {:?}", keys);

        // Verify the file got truncated.
        let size = std::fs::metadata(&path).expect("meta").len();
        assert_eq!(size, 0, "file must be truncated");
    }

    /// Test 8: read_and_compact respects `max_bytes` — write a file bigger than a small
    /// test cap (e.g. `max_bytes: 10`) and confirm not everything past the cap is processed.
    #[test]
    fn read_and_compact_respects_max_bytes() {
        let mut content = String::new();
        // Small valid lines, each big enough to exceed max_bytes=10 after the first line.
        // Each line is ~80 bytes, so max_bytes=10 keeps only the first.
        for i in 0..5 {
            content.push_str(&format!(
                "{{\"cwd\":\"/tmp/a_{i}\",\"at\":\"2024-01-01T00:00:{:02}Z\"}}\n",
                i
            ));
        }
        let path = with_tmp("read_max_bytes.ndjson", &content);

        let cfg = test_config(vec![PathBuf::from("/tmp")]);
        let (result, _counts, _waiting) = read_and_compact(&path, &cfg, 10);

        // With max_bytes=10, the first line (let's see how many bytes it is) is ~46;
        // that exceeds 10 on its own, so... actually let me rethink. bytes_seen=0 at start
        // of loop; line_bytes = stripped.len(); check: bytes_seen(=0) > 0 -> false. So
        // line is always processed if bytes_seen == 0 regardless of size. Then bytes_seen
        // becomes ~46 which is > max_bytes=10. Line 2: bytes_seen(=46)>0 AND (46+line>10) -> break.
        // So exactly one entry should be in the map.
        assert_eq!(
            result.len(),
            1,
            "soft cap should limit to one entry at most, got {:?}",
            result
        );

        // The first line's data must survive (newest "at" wins is irrelevant here).
        let entry = result.values().next().unwrap();
        assert!(
            entry.root.contains("/a_0"),
            "first line should have been processed"
        );

        // And file is truncated.
        let size = std::fs::metadata(&path).expect("meta").len();
        assert_eq!(size, 0);
    }

    /// Test 9: A truncated/incomplete trailing JSON line (write bytes that cut off
    /// mid-object, no trailing newline) -> skipped; the earlier valid lines are still
    /// folded correctly.
    #[test]
    fn read_and_compact_skips_truncated_trailing_line() {
        let content = r#"{"cwd":"/tmp/a","at":"2024-01-01T00:00:01Z","session_id":"a"}
{"cwd":"/tmp/b","at":"2024-01-01T00:00:02Z","session_id":"b"}
{"cwd":"/tmp/c","at":"2024-01-01T00:00:03Z","sess"#;
        let path = with_tmp("read_truncated.ndjson", content);

        let cfg = test_config(vec![PathBuf::from("/tmp")]);
        let (result, _counts, _waiting) = read_and_compact(&path, &cfg, 10_000_000);

        let keys: Vec<&String> = result.keys().collect();
        assert_eq!(
            keys.len(),
            2,
            "truncated trailing line dropped: got {:?}",
            keys
        );

        // File still truncated to empty after read.
        let size = std::fs::metadata(&path).expect("meta").len();
        assert_eq!(size, 0);
    }

    // ═══ MECH-5 — the waiting-on-you deltas ═══════════════════════════════════════════
    //
    // These assert the *third* return value only. The signal fold and the count tally are
    // untouched by this feature, and asserting them here again would only couple these
    // tests to behavior their own name doesn't claim.

    /// Helper: the delta for the root whose path ends with `suffix`. Keys are
    /// `resolve_root`-canonicalized, and on macOS `/tmp` is a symlink to `/private/tmp`, so
    /// looking a fixture root up by the literal string written into the fixture silently
    /// misses — and `Option::None` is a *meaningful* value here, so the miss would read as
    /// a real assertion about the feature rather than as a broken lookup.
    fn delta_for<'a>(waiting: &'a WaitingDeltas, suffix: &str) -> Option<&'a bool> {
        waiting
            .iter()
            .find(|(root, _)| root.ends_with(suffix))
            .map(|(_, v)| v)
    }

    /// Helper: run one pass and return just the waiting deltas.
    fn waiting_for(name: &str, content: &str) -> WaitingDeltas {
        let path = with_tmp(name, content);
        let cfg = test_config(vec![PathBuf::from("/tmp")]);
        let (_signals, _counts, waiting) = read_and_compact(&path, &cfg, 10_000_000);
        waiting
    }

    #[test]
    fn notification_event_sets_waiting() {
        let waiting = waiting_for(
            "waiting_notification.ndjson",
            "{\"cwd\":\"/tmp/w1\",\"at\":\"2024-01-01T00:00:01Z\",\"event\":\"Notification\"}\n",
        );
        assert_eq!(delta_for(&waiting, "/w1"), Some(&true), "got {waiting:?}");
    }

    #[test]
    fn permission_request_event_sets_waiting() {
        let waiting = waiting_for(
            "waiting_permreq.ndjson",
            "{\"cwd\":\"/tmp/w2\",\"at\":\"2024-01-01T00:00:01Z\",\"event\":\"PermissionRequest\"}\n",
        );
        assert_eq!(delta_for(&waiting, "/w2"), Some(&true), "got {waiting:?}");
    }

    #[test]
    fn pre_tool_use_after_notification_clears_waiting() {
        // The whole point of the file-order rule: both lines carry the SAME `at`, because
        // `format_at` writes whole seconds and a human answering a prompt within the same
        // second is entirely ordinary. A timestamp comparison cannot order these; append
        // order can, and it says the human answered.
        let waiting = waiting_for(
            "waiting_cleared_same_second.ndjson",
            "{\"cwd\":\"/tmp/w3\",\"at\":\"2024-01-01T00:00:01Z\",\"event\":\"Notification\"}\n\
             {\"cwd\":\"/tmp/w3\",\"at\":\"2024-01-01T00:00:01Z\",\"event\":\"PreToolUse\"}\n",
        );
        assert_eq!(delta_for(&waiting, "/w3"), Some(&false), "got {waiting:?}");
    }

    #[test]
    fn stop_then_notification_leaves_waiting_set() {
        // The real end-of-turn sequence: `Stop` fires, then Claude Code notifies that it is
        // idle waiting for input. Order matters in both directions, not just the clearing one.
        let waiting = waiting_for(
            "waiting_set_after_stop.ndjson",
            "{\"cwd\":\"/tmp/w4\",\"at\":\"2024-01-01T00:00:01Z\",\"event\":\"Stop\"}\n\
             {\"cwd\":\"/tmp/w4\",\"at\":\"2024-01-01T00:00:02Z\",\"event\":\"Notification\"}\n",
        );
        assert_eq!(delta_for(&waiting, "/w4"), Some(&true), "got {waiting:?}");
    }

    #[test]
    fn unrecognized_event_does_not_clear_waiting() {
        // "Not a clear event" must not be read as a clear. Only the two names in
        // `WAITING_CLEAR_EVENTS` mean the human answered; anything else — an event we never
        // registered, or one Claude Code adds later — says nothing about it.
        let waiting = waiting_for(
            "waiting_unknown_event.ndjson",
            "{\"cwd\":\"/tmp/w5\",\"at\":\"2024-01-01T00:00:01Z\",\"event\":\"Notification\"}\n\
             {\"cwd\":\"/tmp/w5\",\"at\":\"2024-01-01T00:00:02Z\",\"event\":\"PostToolUse\"}\n",
        );
        assert_eq!(delta_for(&waiting, "/w5"), Some(&true), "got {waiting:?}");
    }

    #[test]
    fn root_with_no_relevant_events_is_absent_not_false() {
        // Absent and `false` are different answers: absent means "no news, keep whatever
        // the previous tick decided", `false` means "the human answered, release it". A
        // liveness-only event (or none at all) must produce the former, or every tick with
        // ordinary traffic would silently release a live latch.
        let waiting = waiting_for(
            "waiting_absent.ndjson",
            "{\"cwd\":\"/tmp/w6\",\"at\":\"2024-01-01T00:00:01Z\",\"event\":null}\n",
        );
        assert!(
            delta_for(&waiting, "/w6").is_none(),
            "a root with no waiting-relevant event must not appear at all, got {waiting:?}"
        );
    }

    #[test]
    fn waiting_deltas_are_per_root() {
        let waiting = waiting_for(
            "waiting_per_root.ndjson",
            "{\"cwd\":\"/tmp/w7a\",\"at\":\"2024-01-01T00:00:01Z\",\"event\":\"Notification\"}\n\
             {\"cwd\":\"/tmp/w7b\",\"at\":\"2024-01-01T00:00:01Z\",\"event\":\"Stop\"}\n",
        );
        assert_eq!(delta_for(&waiting, "/w7a"), Some(&true), "got {waiting:?}");
        assert_eq!(delta_for(&waiting, "/w7b"), Some(&false), "got {waiting:?}");
    }
}
