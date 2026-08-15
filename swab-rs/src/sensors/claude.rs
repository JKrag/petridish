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

/// Scan a single transcript file and return the first `sessionId` line and the last
/// `cwd` line (scan order, see F9).
///
/// Uses a tail window for efficiency; falls back to reading the whole file only if no `cwd`
/// was found in that window. Malformed/truncated JSON lines are skipped, never fatal
/// (invariant #4).
fn parse_transcript(file_path: &Path, size: u64) -> (Option<String>, Option<String>) {
    // Plain helper taking explicit `&mut` outparams rather than a capturing closure — a
    // closure that mutably borrows `session_id`/`raw_cwd` would keep that borrow alive for
    // its own lifetime, which conflicts with reading `raw_cwd.is_none()` between the two
    // scan passes below (tail window, then optional full-file fallback).
    fn scan_lines(text: &str, session_id: &mut Option<String>, raw_cwd: &mut Option<String>) {
        // Mirrors the Python `_scan_lines`: first-hit `sessionId`, last-hit `cwd`.
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
            if session_id.is_none() {
                if let Some(Value::String(s)) = obj.get("sessionId") {
                    *session_id = Some(s.clone());
                }
            }
            // `cwd` always takes the LAST line that carries one (F9).
            if let Some(Value::String(s)) = obj.get("cwd") {
                *raw_cwd = Some(s.clone());
            }
        }
    }

    let mut session_id: Option<String> = None;
    let mut raw_cwd: Option<String> = None;

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
            scan_lines(&body, &mut session_id, &mut raw_cwd);
        }
    }

    // Fallback: if we got no cwd at all from the tail window, re-read the whole file.
    // This is rare (most transcripts have a cwd near the end) but keeps the sensor honest for
    // very small or oddly structured transcripts. Matches Python's `if raw_cwd is None` branch.
    if raw_cwd.is_none() {
        match fs::read_to_string(file_path) {
            Ok(text) => scan_lines(&text, &mut session_id, &mut raw_cwd),
            Err(_) => return (session_id, None),
        }
    }

    (session_id, raw_cwd)
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

            // Parse the transcript: first-hit sessionId, last-hit cwd (F9).
            let (session_id, raw_cwd) = parse_transcript(&file_path, size);

            // If no `cwd` was found anywhere in the file (tail nor full fallback): skip entirely
            // — no signal, no panic, just contribute nothing (test #10).
            let Some(ref raw_cwd) = raw_cwd else {
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
                    session_id,
                    event: None, // only the hook path sets this.
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
            let path = std::env::temp_dir().join(format!("swab_rs_claude_sensor_{suffix}"));
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
}
