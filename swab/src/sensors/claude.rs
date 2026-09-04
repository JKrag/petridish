//! Claude Code transcript sensor. Mirrors `src/petridish/sensors/claude.py`.
//! The primary implementation of invariants #2–#4 (`CLAUDE.md`) — read these verbatim
//! findings from `ARCHITECTURE.md` §0 before writing this module's logic:
//!
//! - F2: slug is not reversibly decodable — `cwd` is read from JSONL content, NEVER the
//!   `~/.claude/projects/<slug>/` dirname.
//! - F9: `cwd` varies within one transcript — take it from the **last** line (in file-scan
//!   order) that carries a `cwd` field, then resolve via `discovery::resolve_root`.
//! - F3 / invariant #3: liveness = file **mtime** recency, not any timestamp inside the
//!   JSON — there is no reliable "working vs idle" state machine from transcript content.
//! - Invariant #4: truncated trailing JSONL lines are normal; skip and fall back, never error.

use crate::config::Config;
use crate::discovery;
use crate::schema::AgentSignal;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::Path;

/// Bytes to seek back from EOF before scanning forward (tail-read optimization) — the first
/// (likely partial) line after the seek is discarded. Falls back to reading the whole file
/// from the top only if no `cwd` was found in the tail window.
pub const TAIL_BYTES: u64 = 65_536;

/// Directories under `~/.claude/projects/` whose transcripts haven't been touched in this
/// many hours are skipped before even opening the file (cold-skip).
pub const DEFAULT_COLD_CUTOFF_HOURS: u64 = 1_440;

/// The facts one transcript file contributes: first-hit `sessionId`, last-hit `cwd` (F9),
/// and the last recognized conversational event name (see `event_name_for`).
#[derive(Debug, Default)]
struct TranscriptFacts {
    session_id: Option<String>,
    raw_cwd: Option<String>,
    event: Option<String>,
    /// Whether `event` is a *weak* name — see `DerivedEvent::weak`. Internal to
    /// the scan; never leaves this module.
    event_is_weak: bool,
}

/// Scan a single transcript file and return its `TranscriptFacts`: first-hit `sessionId`,
/// last-hit `cwd` (scan order, see F9), and the last recognized conversational event name.
///
/// Uses a tail window for efficiency; falls back to reading the whole file only if no `cwd`
/// was found in that window. Malformed/truncated JSON lines are skipped, never fatal
/// (invariant #4). The event name is derived on the same pass that collects the other facts —
/// never a separate re-read — so only a missing `cwd` can ever trigger the fallback.
fn parse_transcript(file_path: &Path, size: u64) -> TranscriptFacts {
    // Plain helper taking explicit `&mut` outparams rather than a capturing closure — a
    // closure that mutably borrows `session_id`/`raw_cwd` would keep that borrow alive for
    // its own lifetime, which conflicts with reading `raw_cwd.is_none()` between the two
    // scan passes below (tail window, then optional full-file fallback).
    fn scan_lines(text: &str, facts: &mut TranscriptFacts) {
        // Mirrors the Python `_scan_lines`: first-hit `sessionId`, last-hit `cwd` (F9), and the
        // last recognized event name.
        for line in text.split('\n') {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                // Trailing partial JSON — normal in live sessions. Skip and keep going rather
                // than aborting the whole file (invariant #4).
                continue;
            };
            // Must be a JSON object (Claude transcripts emit JSONL objects).
            let Some(obj) = record.as_object() else {
                continue;
            };
            // `session_id` from the FIRST line that carries one (Python: freeze after first).
            if facts.session_id.is_none() {
                if let Some(Value::String(s)) = obj.get("sessionId") {
                    facts.session_id = Some(s.clone());
                }
            }
            // `cwd` always takes the LAST line that carries one (F9).
            if let Some(Value::String(s)) = obj.get("cwd") {
                facts.raw_cwd = Some(s.clone());
            }
            // Derive a conversational event name from this record. Last-recognized-wins,
            // same spirit as `cwd`'s last-hit-wins, with one exception: the agent's own
            // closing prose (`weak`) fills an empty slot but never displaces a name already
            // there. Without that exception a turn shaped "run Bash, then say what it found"
            // reports "assistant message", which is the shape of nearly every turn. An
            // unrecognized record leaves `event` untouched either way, so a stretch of
            // internal bookkeeping records can't erase the last real event name.
            if let Some(ev) = event_name_for(obj) {
                if !ev.weak || facts.event.is_none() || facts.event_is_weak {
                    facts.event = Some(ev.name);
                    facts.event_is_weak = ev.weak;
                }
            }
        }
    }

    let mut facts: TranscriptFacts = TranscriptFacts::default();

    // Tail-read optimization. If the file is smaller than `TAIL_BYTES`, this is all we do.
    if let Ok(mut fh) = fs::File::open(file_path) {
        let seek_pos = size.saturating_sub(TAIL_BYTES);
        // seek + drop-first-partial-line, but degrade to full-file fallback on any I/O error.
        if fh.seek(SeekFrom::Start(seek_pos)).is_ok() {
            // `File` only implements `Read`, not `BufRead` — wrap it so `read_line` (a
            // `BufRead` method) has something to call it on. Seeking before wrapping means
            // there's no stale buffered data to worry about.
            let mut reader = std::io::BufReader::new(fh);
            if seek_pos > 0 {
                // The first line after the seek lands mid-record; drop it so a mid-record
                // seek doesn't leave us holding a dangling half-object.
                let mut buffer = String::new();
                let _ = reader.read_line(&mut buffer);
            }
            // Scan every complete line in the tail window.
            let mut body = String::new();
            let _ = reader.read_to_string(&mut body);
            scan_lines(&body, &mut facts);
        }
    }

    // Fallback: if we got no cwd at all from the tail window, re-read the whole file.
    // This is rare (most transcripts have a cwd near the end) but keeps the sensor honest for
    // very small or oddly structured transcripts. Matches Python's `if raw_cwd is None` branch.
    // Only re-reads for the missing cwd — the event is populated on the pass above, not here.
    if facts.raw_cwd.is_none() {
        match fs::read_to_string(file_path) {
            Ok(text) => scan_lines(&text, &mut facts),
            Err(_) => return facts,
        }
    }

    facts
}

/// An event name plus how firmly it holds the slot.
///
/// The distinction exists because "last recognized record wins" is the wrong rule
/// on its own. An agent's turn almost always ends with prose — it runs `Bash`, then
/// replies about what it found — so plain last-wins reported "assistant message" for
/// nearly every project, which is true and says nothing the row didn't already say by
/// naming the project. A tool name is the informative thing, and it must not be
/// displaced by the agent's own closing sentence.
struct DerivedEvent {
    name: String,
    /// `true` only for the agent's own prose. A weak name fills an empty slot and
    /// may be replaced by anything; it never overwrites a name already there.
    /// A *user* prompt is deliberately strong: it starts a new turn, so it is real
    /// news rather than a trailing remark about work already reported.
    weak: bool,
}

/// Maps one parsed JSONL record to a human event name, or `None` if the record isn't a
/// recognized conversational record. Last-recognized-wins across a file (see `scan_lines`); an
/// unrecognized record degrades to `None`, which the TUI renders as the old generic "activity"
/// text rather than a garbage name.
///
/// Recognized so far: an `assistant` record whose content ends in a `tool_use` (its `name` is
/// the event, e.g. "Bash"/"Edit"/"WebFetch"), or a `user` record whose content isn't a
/// mechanical `tool_result` echo. Everything else — attachments, mode changes, internal
/// bookkeeping records, and any future Claude Code record type — returns `None`. This is an
/// allowlist on purpose: real transcripts are full of unmodeled records, and a new release can
/// add more, and none of them should leak a meaningless label into a single-line row.
fn event_name_for(obj: &serde_json::Map<String, Value>) -> Option<DerivedEvent> {
    // Case A — assistant. The event is the last `tool_use` in the content array (the
    // assistant's final action). No `tool_use` but a non-empty array → "assistant message"; a
    // string content is the same. Missing, null, or empty content yields nothing.
    if obj.get("type").and_then(Value::as_str) == Some("assistant") {
        if let Some(Value::Array(items)) = obj.get("message")
            .and_then(|m| m.get("content")) {
            for item in items.iter().rev() {
                let Some(map) = item.as_object() else { continue };
                if map.get("type").and_then(Value::as_str) != Some("tool_use") {
                    continue;
                }
                if let Some(Value::String(name)) = map.get("name") {
                    return sanitize_tool_name(name).map(|name| DerivedEvent { name, weak: false });
                }
            }
            // Array had no tool_use — but it may be empty, which per the rules is None.
            if !items.is_empty() {
                return Some(DerivedEvent { name: "assistant message".to_string(), weak: true });
            }
            return None;
        }
        // A string content is still a message; a missing or null one carries nothing to
        // report and must not be reported as activity that didn't happen.
        return match obj.get("message").and_then(|m| m.get("content")) {
            Some(Value::String(_)) => {
                Some(DerivedEvent { name: "assistant message".to_string(), weak: true })
            }
            _ => None,
        };
    }

    // Case B — user. A `tool_result` in the content is the mechanical echo of a tool that
    // already produced its name in case A; don't let it override. Anything else (a string, or an
    // array with no tool_result) is a genuine user prompt.
    if obj.get("type").and_then(Value::as_str) == Some("user") {
        let echoes_a_tool = matches!(
            obj.get("message").and_then(|m| m.get("content")),
            Some(Value::Array(items)) if items.iter().any(|item| {
                matches!(item, Value::Object(map) if map.get("type").and_then(Value::as_str) == Some("tool_result"))
            })
        );
        if echoes_a_tool {
            return None;
        }
        return Some(DerivedEvent { name: "user prompt".to_string(), weak: false });
    }

    // Case C — anything else (attachment, mode, permission-mode, atis-latch, bridge-session,
    // last-prompt, ai-title, file-history-snapshot, a missing type, or any future type) is
    // unmodeled. Degrade to None rather than invent a label from it.
    None
}

/// Sanitizes a `tool_use` name before it becomes a single-line row label. Returns `None` when
/// the name can't render as a label — empty, longer than 40 characters, or containing a
/// control character (e.g. a newline) — rather than letting it corrupt the layout. The two
/// literal strings ("assistant message", "user prompt") need no check.
///
/// The limit counts **characters, not bytes**: byte length would reject a short name written
/// in any non-Latin script purely for being encoded in more than one byte per character.
/// Characters are not the same as terminal columns either — a wide character occupies two —
/// but this is a guard against an absurd name corrupting a row, not a layout calculation, and
/// the real inputs are ASCII tool identifiers (`Bash`, `WebFetch`, `mcp__server__tool`) where
/// all three measures agree. `petri` does the actual column math at render time.
fn sanitize_tool_name(name: &str) -> Option<String> {
    if name.is_empty() || name.chars().count() > 40 || name.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(name.to_string())
}

/// Walks `claude_projects_dir` (`~/.claude/projects/*/*.jsonl`), building one `AgentSignal`
/// per resolved project root. Per transcript file: `session_id` = first line (scan order)
/// with a `sessionId` string field; `cwd` = last line (scan order) with a `cwd` string field
/// (F9), resolved via `discovery::resolve_root`; `at` = the file's mtime (F3), not any JSON
/// timestamp. Cold files (mtime older than `cold_cutoff_hours`) are skipped without opening.
/// Malformed/truncated JSON lines are caught and skipped, never fatal (invariant #4). Two
/// transcripts resolving to the same root fold to one signal, newest file mtime wins. A
/// missing `claude_projects_dir` returns an empty map, never an error (invariant #5).
pub fn scan(
    claude_projects_dir: &Path,
    config: &Config,
    cold_cutoff_hours: u64,
) -> HashMap<String, AgentSignal> {
    // Missing claude_projects_dir → empty map, never an error (invariant #5).
    // `read_dir` returns Err for both missing and unreadable dirs — treat both the same:
    // degrade silently.
    let Ok(dir_entries) = fs::read_dir(claude_projects_dir) else {
        return HashMap::new();
    };

    // Seconds since `now` for the cutoff — computed once so a slow file walk doesn't
    // recompute it on every stat(). Python does `now_ts - cold_cutoff_hours * 3600`.
    let cutoff_secs = cold_cutoff_hours as i64 * 3_600;
    let cutoff_ts = Utc::now() - chrono::Duration::seconds(cutoff_secs);

    let mut signals: HashMap<String, AgentSignal> = HashMap::new();

    for dir_entry in dir_entries.flatten() {
        let path = dir_entry.path();
        // Only recurse into subdirectories (each is a "slug" dir).
        if !path.is_dir() {
            continue;
        }

        // List children inside this slug dir with the whole block inside a try/except —
        // `dir_entry`'s own error would already be flattened above; this is for inner unreadable
        // dirs / permission errors (invariant: "sensors degrade, never abort").
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };

        for entry in entries.flatten() {
            let file_path = entry.path();
            // Only .jsonl files (skip stray .txt, etc. — test #11).
            if !file_path.is_file() {
                continue;
            }
            let Some(ext) = file_path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if ext != "jsonl" {
                continue;
            }

            // `metadata` call: if the file vanished between this stat and open, degrade
            // silently — never abort the whole scan.
            let Ok(meta) = fs::metadata(&file_path) else {
                continue;
            };

            // Cold-skip: if the file's mtime is older than `cold_cutoff_hours` hours ago,
            // skip it WITHOUT opening (invariant: never parse a path out of the slug).
            let Ok(mtime) = meta.modified() else {
                continue;
            };
            let mtime_as_dt = match DateTime::from_timestamp(
                mtime.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64,
                mtime.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_nanos(),
            ) {
                Some(dt) => dt,
                None => continue, // timestamp out of range — degrade to skip.
            };

            if mtime_as_dt < cutoff_ts {
                continue;
            }

            // Compute file-mtime as DateTime<Utc> for the signal's `at` field. F3 — file mtime,
            // not any timestamp inside the JSON content. Python uses `datetime.fromtimestamp`
            // on `st.st_mtime`.
            let file_mtime = mtime_as_dt;
            let size = meta.len();

            // Parse the transcript: first-hit sessionId, last-hit cwd (F9), last-recognized
            // event name.
            let facts = parse_transcript(&file_path, size);

            // If no `cwd` was found anywhere in the file (tail nor full fallback): skip entirely
            // — no signal, no panic, just contribute nothing (test #10).
            let Some(ref raw_cwd) = facts.raw_cwd else {
                continue;
            };

            // Resolve via `discovery::resolve_root` (F9 — every raw cwd must be resolved up to
            // its enclosing project root, or one monorepo session shatters into phantom projects).
            let resolved = discovery::resolve_root(&std::path::PathBuf::from(raw_cwd), config);
            let root_key = resolved.to_string_lossy().into_owned();

            // Two transcripts resolving to the same root: the one with the NEWER file mtime
            // wins (Python's `if existing is not None and existing.at >= file_mtime: continue`).
            if let Some(existing) = signals.get(&root_key) {
                if existing.at >= file_mtime {
                    continue;
                }
            }

            signals.insert(
                root_key,
                AgentSignal {
                    root: resolved.to_string_lossy().into_owned(),
                    at: file_mtime,
                    agent: String::from("claude-code"),
                    session_id: facts.session_id,
                    // The transcript sensor derives this now (see `event_name_for`); the
                    // hook path supplies one too, and the newest signal wins in `scan`.
                    event: facts.event,
                    raw_cwd: Some(raw_cwd.clone()),
                },
            );
        }
    }

    signals
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::time::{Duration, SystemTime};

    fn test_config(roots: Vec<std::path::PathBuf>) -> Config {
        let mut cfg = Config::default();
        cfg.roots = roots;
        cfg.extra_paths = vec![];
        cfg.ignore_dirs = HashSet::new();
        cfg
    }

    /// Fresh temp dir, cleaned on drop.
    struct Tmp {
        path: std::path::PathBuf,
    }
    impl Tmp {
        fn new(suffix: &str) -> Self {
            let path = std::env::temp_dir().join(format!("swab_claude_sensor_{suffix}_{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("mktemp");
            Self { path }
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn init_repo(path: &Path) {
        fs::create_dir_all(path.join(".git")).expect("mkdir .git");
    }

    /// Writes `lines` (each already-serialized as a JSON string) as one `.jsonl` transcript,
    /// then sets its mtime to `seconds_ago` seconds before now (default: fresh/now if 0).
    fn write_transcript(path: &Path, lines: &[String], seconds_ago: u64) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir transcript dir");
        }
        let body = lines.join("\n") + "\n";
        fs::write(path, body).expect("write transcript");
        let mtime = SystemTime::now() - Duration::from_secs(seconds_ago);
        let file = fs::File::open(path).expect("reopen for mtime");
        file.set_modified(mtime).expect("set mtime");
    }

    fn line(session_id: Option<&str>, cwd: Option<&str>) -> String {
        let mut obj = serde_json::Map::new();
        if let Some(s) = session_id {
            obj.insert("sessionId".into(), Value::String(s.to_string()));
        }
        if let Some(c) = cwd {
            obj.insert("cwd".into(), Value::String(c.to_string()));
        }
        serde_json::to_string(&Value::Object(obj)).unwrap()
    }

    // 1. One transcript -> one signal for that root.
    #[test]
    fn one_transcript_yields_one_signal() {
        let tmp = Tmp::new("one_signal");
        let repo = tmp.path.join("repo");
        init_repo(&repo);

        let projects_dir = tmp.path.join("claude_projects");
        let slug = projects_dir.join("-some-slug");
        write_transcript(
            &slug.join("session-1.jsonl"),
            &[line(Some("sess-1"), Some(repo.to_str().unwrap()))],
            0,
        );

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&projects_dir, &cfg, DEFAULT_COLD_CUTOFF_HOURS);

        assert_eq!(signals.len(), 1);
        let sig = signals.values().next().unwrap();
        assert_eq!(sig.session_id.as_deref(), Some("sess-1"));
        assert_eq!(sig.agent, "claude-code");
    }

    // 2. The slug dirname must never be parsed for a path (F2) — a dash-heavy fake path in
    // the dirname must not influence the resolved root, only the JSONL `cwd` content does.
    #[test]
    fn slug_dirname_is_never_parsed_for_a_path() {
        let tmp = Tmp::new("slug_ignored");
        let repo = tmp.path.join("realrepo");
        init_repo(&repo);

        let projects_dir = tmp.path.join("claude_projects");
        // Slug looks like it could decode to some other, nonexistent path -- must be ignored.
        let slug = projects_dir.join("-Users-someone-repos-decoy-project");
        write_transcript(
            &slug.join("session-1.jsonl"),
            &[line(Some("sess-2"), Some(repo.to_str().unwrap()))],
            0,
        );

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&projects_dir, &cfg, DEFAULT_COLD_CUTOFF_HOURS);

        assert_eq!(signals.len(), 1);
        let root = signals.keys().next().unwrap();
        assert!(root.contains("realrepo"), "root must come from JSONL cwd, got {root:?}");
        assert!(!root.contains("decoy"), "root must not be derived from the slug dirname");
    }

    // 3. `cwd` changes mid-file -> the LAST value wins (F9).
    #[test]
    fn cwd_changes_mid_file_last_wins() {
        let tmp = Tmp::new("cwd_last_wins");
        let repo_a = tmp.path.join("repo_a");
        let repo_b = tmp.path.join("repo_b");
        init_repo(&repo_a);
        init_repo(&repo_b);

        let projects_dir = tmp.path.join("claude_projects");
        let slug = projects_dir.join("-slug");
        write_transcript(
            &slug.join("session-1.jsonl"),
            &[
                line(Some("sess-3"), Some(repo_a.to_str().unwrap())),
                line(None, Some(repo_b.to_str().unwrap())),
            ],
            0,
        );

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&projects_dir, &cfg, DEFAULT_COLD_CUTOFF_HOURS);

        assert_eq!(signals.len(), 1);
        let root = signals.keys().next().unwrap();
        assert!(root.contains("repo_b"), "last cwd (repo_b) must win, got {root:?}");
    }

    // 4. Monorepo: cwd points at a subdir -> resolves to the repo root via resolve_root.
    #[test]
    fn monorepo_subdir_cwd_resolves_to_repo_root() {
        let tmp = Tmp::new("monorepo");
        let repo = tmp.path.join("monorepo");
        init_repo(&repo);
        let subdir = repo.join("packages").join("core");
        fs::create_dir_all(&subdir).expect("mkdir subdir");

        let projects_dir = tmp.path.join("claude_projects");
        let slug = projects_dir.join("-slug");
        write_transcript(
            &slug.join("session-1.jsonl"),
            &[line(Some("sess-4"), Some(subdir.to_str().unwrap()))],
            0,
        );

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&projects_dir, &cfg, DEFAULT_COLD_CUTOFF_HOURS);

        assert_eq!(signals.len(), 1);
        let root = signals.keys().next().unwrap();
        let resolved = std::path::PathBuf::from(root).canonicalize().unwrap();
        let expected = repo.canonicalize().unwrap();
        assert_eq!(resolved, expected);
    }

    // 5. A truncated final line must not abort the file; valid lines before it still count.
    #[test]
    fn truncated_trailing_line_does_not_abort() {
        let tmp = Tmp::new("truncated");
        let repo = tmp.path.join("repo");
        init_repo(&repo);

        let projects_dir = tmp.path.join("claude_projects");
        let slug = projects_dir.join("-slug");
        let path = slug.join("session-1.jsonl");
        fs::create_dir_all(&slug).expect("mkdir slug");
        let good = line(Some("sess-5"), Some(repo.to_str().unwrap()));
        let body = format!("{good}\n{{\"message\": \"cut off mid-writ");
        fs::write(&path, body).expect("write transcript");
        let mtime = SystemTime::now();
        fs::File::open(&path).unwrap().set_modified(mtime).unwrap();

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&projects_dir, &cfg, DEFAULT_COLD_CUTOFF_HOURS);

        assert_eq!(signals.len(), 1, "the valid line before the truncated one must still count");
    }

    // 6. A cold file (mtime older than the cutoff) is skipped entirely.
    #[test]
    fn cold_file_is_skipped() {
        let tmp = Tmp::new("cold");
        let repo = tmp.path.join("repo");
        init_repo(&repo);

        let projects_dir = tmp.path.join("claude_projects");
        let slug = projects_dir.join("-slug");
        // 2000 hours ago, well past a 24h cutoff.
        write_transcript(
            &slug.join("session-1.jsonl"),
            &[line(Some("sess-6"), Some(repo.to_str().unwrap()))],
            2000 * 3600,
        );

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&projects_dir, &cfg, 24);

        assert!(signals.is_empty(), "cold file must not contribute a signal");
    }

    // 7. Two transcripts resolving to the same root -> the newer-mtime one wins.
    #[test]
    fn same_root_newest_mtime_wins() {
        let tmp = Tmp::new("newest_wins");
        let repo = tmp.path.join("repo");
        init_repo(&repo);

        let projects_dir = tmp.path.join("claude_projects");
        write_transcript(
            &projects_dir.join("-slug-old").join("session-old.jsonl"),
            &[line(Some("sess-old"), Some(repo.to_str().unwrap()))],
            300, // 5 minutes ago
        );
        write_transcript(
            &projects_dir.join("-slug-new").join("session-new.jsonl"),
            &[line(Some("sess-new"), Some(repo.to_str().unwrap()))],
            0, // just now
        );

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&projects_dir, &cfg, DEFAULT_COLD_CUTOFF_HOURS);

        assert_eq!(signals.len(), 1);
        let sig = signals.values().next().unwrap();
        assert_eq!(sig.session_id.as_deref(), Some("sess-new"), "newer mtime must win");
    }

    // 8. Empty claude_projects_dir (exists, no subdirs) -> empty map, no panic.
    #[test]
    fn empty_projects_dir_yields_empty_map() {
        let tmp = Tmp::new("empty_dir");
        let projects_dir = tmp.path.join("claude_projects");
        fs::create_dir_all(&projects_dir).expect("mkdir");

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&projects_dir, &cfg, DEFAULT_COLD_CUTOFF_HOURS);

        assert!(signals.is_empty());
    }

    // 9. Missing claude_projects_dir (doesn't exist on disk) -> empty map, no panic.
    #[test]
    fn missing_projects_dir_yields_empty_map() {
        let tmp = Tmp::new("missing_dir");
        let projects_dir = tmp.path.join("does_not_exist");

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&projects_dir, &cfg, DEFAULT_COLD_CUTOFF_HOURS);

        assert!(signals.is_empty());
    }

    // 10. A transcript with no `cwd` field on any line contributes no signal.
    #[test]
    fn no_cwd_anywhere_yields_no_signal() {
        let tmp = Tmp::new("no_cwd");
        let projects_dir = tmp.path.join("claude_projects");
        let slug = projects_dir.join("-slug");
        write_transcript(
            &slug.join("session-1.jsonl"),
            &[line(Some("sess-10"), None), line(None, None)],
            0,
        );

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&projects_dir, &cfg, DEFAULT_COLD_CUTOFF_HOURS);

        assert!(signals.is_empty());
    }

    // 11. A stray non-.jsonl file in a slug dir is ignored, doesn't crash the scan.
    #[test]
    fn non_jsonl_file_is_ignored() {
        let tmp = Tmp::new("non_jsonl");
        let repo = tmp.path.join("repo");
        init_repo(&repo);

        let projects_dir = tmp.path.join("claude_projects");
        let slug = projects_dir.join("-slug");
        fs::create_dir_all(&slug).expect("mkdir slug");
        fs::write(slug.join("notes.txt"), "not a transcript\n").expect("write stray file");
        write_transcript(
            &slug.join("session-1.jsonl"),
            &[line(Some("sess-11"), Some(repo.to_str().unwrap()))],
            0,
        );

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&projects_dir, &cfg, DEFAULT_COLD_CUTOFF_HOURS);

        assert_eq!(signals.len(), 1, "stray non-.jsonl file must not produce a signal or crash the scan");
    }

    /// Builds one typed transcript record: `{"type": <record_type>, "cwd": ..., "message":
    /// {"content": <content>}}`. Separate from `line` so the existing tests' minimal
    /// `sessionId`/`cwd` records stay exactly as they were.
    fn typed_line(cwd: Option<&str>, record_type: &str, content: Option<Value>) -> String {
        let mut obj = serde_json::Map::new();
        obj.insert("type".into(), Value::String(record_type.to_string()));
        if let Some(c) = cwd {
            obj.insert("cwd".into(), Value::String(c.to_string()));
        }
        if let Some(content) = content {
            let mut message = serde_json::Map::new();
            message.insert("content".into(), content);
            obj.insert("message".into(), Value::Object(message));
        }
        serde_json::to_string(&Value::Object(obj)).unwrap()
    }

    /// A `tool_use` content array carrying one tool `name`.
    fn tool_use(name: &str) -> Value {
        serde_json::json!([{"type": "tool_use", "name": name, "input": {}}])
    }

    /// Runs `scan` over a one-transcript fixture and returns that signal's derived `event`.
    /// Every event test below goes through the real `scan` entry point rather than calling
    /// `parse_transcript` directly, so the wiring at the call site is covered too — the
    /// field was once populated with the `cwd` instead of the event name, and a test that
    /// stopped at `parse_transcript` would not have seen it.
    fn event_from(suffix: &str, lines: &[String]) -> Option<String> {
        let tmp = Tmp::new(suffix);
        let repo = tmp.path.join("repo");
        init_repo(&repo);
        let repo_str = repo.to_str().unwrap().to_string();

        // Every fixture line's `cwd` placeholder is filled in with this tmpdir's real repo.
        let filled: Vec<String> = lines
            .iter()
            .map(|l| l.replace("__CWD__", &repo_str))
            .collect();

        let projects_dir = tmp.path.join("claude_projects");
        write_transcript(&projects_dir.join("-slug").join("s.jsonl"), &filled, 0);

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&projects_dir, &cfg, DEFAULT_COLD_CUTOFF_HOURS);
        assert_eq!(signals.len(), 1, "fixture must yield exactly one signal");
        signals.values().next().unwrap().event.clone()
    }

    // 12. An assistant `tool_use` record names the tool it invoked — the most informative
    // event this sensor can report, and the one a live run produces.
    #[test]
    fn assistant_tool_use_yields_the_tool_name() {
        let event = event_from(
            "ev_tool_use",
            &[typed_line(Some("__CWD__"), "assistant", Some(tool_use("Bash")))],
        );
        assert_eq!(event.as_deref(), Some("Bash"));
    }

    // 13. An assistant record with only prose carries no tool name, but is still real
    // activity — the shape a finished turn leaves behind.
    #[test]
    fn assistant_text_only_yields_assistant_message() {
        let event = event_from(
            "ev_text",
            &[typed_line(
                Some("__CWD__"),
                "assistant",
                Some(serde_json::json!([{"type": "text", "text": "hi"}])),
            )],
        );
        assert_eq!(event.as_deref(), Some("assistant message"));
    }

    // 14. A user record whose content is a bare string is a genuine prompt.
    #[test]
    fn user_string_content_yields_user_prompt() {
        let event = event_from(
            "ev_prompt",
            &[typed_line(
                Some("__CWD__"),
                "user",
                Some(Value::String("do the thing".into())),
            )],
        );
        assert_eq!(event.as_deref(), Some("user prompt"));
    }

    // 15. A `tool_result` is the mechanical echo of a tool call that already reported its own
    // name. Letting it win would make every live session read "tool result" instead of the
    // tool that actually ran.
    #[test]
    fn tool_result_does_not_override_the_tool_name() {
        let event = event_from(
            "ev_tool_result",
            &[
                typed_line(Some("__CWD__"), "assistant", Some(tool_use("Bash"))),
                typed_line(
                    Some("__CWD__"),
                    "user",
                    Some(serde_json::json!([{"type": "tool_result", "content": "ok"}])),
                ),
            ],
        );
        assert_eq!(event.as_deref(), Some("Bash"));
    }

    // 16. Real transcripts end on a run of internal bookkeeping records. Those are not
    // activity and must not erase the last real event name.
    #[test]
    fn unknown_record_types_do_not_clobber_a_real_event() {
        let event = event_from(
            "ev_unknown_tail",
            &[
                typed_line(Some("__CWD__"), "assistant", Some(tool_use("Edit"))),
                typed_line(Some("__CWD__"), "bridge-session", None),
                typed_line(Some("__CWD__"), "atis-latch", None),
                typed_line(Some("__CWD__"), "ai-title", None),
            ],
        );
        assert_eq!(event.as_deref(), Some("Edit"));
    }

    // 17. Degradation, not omission: a transcript of nothing but unmodeled record types still
    // produces its signal (the project is real and the mtime is still liveness), with no
    // event name rather than an invented one.
    #[test]
    fn transcript_of_only_unknown_types_yields_no_event() {
        let event = event_from(
            "ev_all_unknown",
            &[
                typed_line(Some("__CWD__"), "bridge-session", None),
                typed_line(Some("__CWD__"), "file-history-snapshot", None),
            ],
        );
        assert_eq!(event, None);
    }

    // 18. Last recognized record wins, same spirit as `cwd`'s last-hit-wins (F9).
    #[test]
    fn last_recognized_event_wins() {
        let event = event_from(
            "ev_last_wins",
            &[
                typed_line(Some("__CWD__"), "assistant", Some(tool_use("Read"))),
                typed_line(Some("__CWD__"), "assistant", Some(tool_use("Bash"))),
            ],
        );
        assert_eq!(event.as_deref(), Some("Bash"));
    }

    // 19. A tool name is rendered into a single-line terminal row. An overlong one or one
    // carrying a newline would corrupt the layout, so it is dropped rather than truncated —
    // the signal itself must still survive.
    #[test]
    fn oversized_or_control_char_tool_name_is_rejected() {
        let overlong = "x".repeat(41);
        let event = event_from(
            "ev_overlong",
            &[typed_line(
                Some("__CWD__"),
                "assistant",
                Some(tool_use(&overlong)),
            )],
        );
        assert_eq!(event, None, "a 41-character tool name must be rejected");

        let event = event_from(
            "ev_control_char",
            &[typed_line(
                Some("__CWD__"),
                "assistant",
                Some(tool_use("Ba\nsh")),
            )],
        );
        assert_eq!(event, None, "a newline in a tool name must be rejected");
    }

    // 20. Invariant #4: a live session is being appended to as we read it, so the last line
    // is routinely half-written. That must not cost us the event name from the line before.
    #[test]
    fn truncated_trailing_line_does_not_lose_the_event() {
        let mut lines = vec![typed_line(
            Some("__CWD__"),
            "assistant",
            Some(tool_use("Bash")),
        )];
        lines.push("{\"type\": \"assis".to_string()); // truncated mid-write
        let event = event_from("ev_truncated", &lines);
        assert_eq!(event.as_deref(), Some("Bash"));
    }

    // 21. The event is derived on the same pass that collects the `cwd` — a transcript whose
    // event sits on the first line and whose `cwd` sits on the last still reports both,
    // without the event ever being the reason for a whole-file re-read.
    #[test]
    fn event_name_survives_the_whole_file_fallback() {
        let event = event_from(
            "ev_first_line",
            &[
                typed_line(None, "assistant", Some(tool_use("Bash"))),
                line(Some("sess-1"), Some("__CWD__")),
            ],
        );
        assert_eq!(event.as_deref(), Some("Bash"));
    }

    // 21b. The length limit counts characters, not bytes — a name written in a
    // non-Latin script must not be rejected merely for encoding to more bytes than
    // it has characters. 40 three-byte characters is 120 bytes: accepted on the
    // documented rule, rejected by a byte count.
    #[test]
    fn the_length_limit_counts_characters_not_bytes() {
        let forty_wide: String = "あ".repeat(40);
        assert_eq!(forty_wide.chars().count(), 40);
        assert_eq!(forty_wide.len(), 120, "fixture must be multi-byte per character");

        let event = event_from(
            "ev_multibyte_name",
            &[typed_line(
                Some("__CWD__"),
                "assistant",
                Some(tool_use(&forty_wide)),
            )],
        );
        assert_eq!(event.as_deref(), Some(forty_wide.as_str()));

        // One character over the limit is still rejected, on the same measure.
        let forty_one: String = "あ".repeat(41);
        let event = event_from(
            "ev_multibyte_too_long",
            &[typed_line(
                Some("__CWD__"),
                "assistant",
                Some(tool_use(&forty_one)),
            )],
        );
        assert_eq!(event, None);
    }

    // 22. The informative half of the fix: a turn almost always ends with the agent
    // saying what it did, so plain last-wins reported "assistant message" for nearly
    // every project. The tool that ran is the thing worth showing.
    #[test]
    fn a_tool_name_survives_the_agents_closing_prose() {
        let event = event_from(
            "ev_prose_after_tool",
            &[
                typed_line(Some("__CWD__"), "assistant", Some(tool_use("Bash"))),
                typed_line(
                    Some("__CWD__"),
                    "assistant",
                    Some(serde_json::json!([{"type": "text", "text": "that worked"}])),
                ),
            ],
        );
        assert_eq!(event.as_deref(), Some("Bash"));
    }

    // 23. The other half: a *user* prompt is a new turn, not a trailing remark about
    // work already reported, so it does replace the tool name. Otherwise a project
    // would keep reporting a tool it ran before the human last spoke.
    #[test]
    fn a_new_user_prompt_replaces_the_tool_name() {
        let event = event_from(
            "ev_prompt_after_tool",
            &[
                typed_line(Some("__CWD__"), "assistant", Some(tool_use("Bash"))),
                typed_line(
                    Some("__CWD__"),
                    "assistant",
                    Some(serde_json::json!([{"type": "text", "text": "done"}])),
                ),
                typed_line(
                    Some("__CWD__"),
                    "user",
                    Some(Value::String("now do the other thing".into())),
                ),
            ],
        );
        assert_eq!(event.as_deref(), Some("user prompt"));
    }

    // 24. A weak name fills an empty slot, and a later strong one takes it over.
    #[test]
    fn prose_fills_an_empty_slot_and_a_later_tool_takes_it() {
        let event = event_from(
            "ev_prose_then_tool",
            &[
                typed_line(
                    Some("__CWD__"),
                    "assistant",
                    Some(serde_json::json!([{"type": "text", "text": "let me look"}])),
                ),
                typed_line(Some("__CWD__"), "assistant", Some(tool_use("Read"))),
            ],
        );
        assert_eq!(event.as_deref(), Some("Read"));
    }
}
