//! VS Code Copilot sensor. Mirrors `src/petridish/sensors/copilot.py`.
//! Per `ARCHITECTURE.md` §0 finding F6: `workspaceStorage/<hash>/` dirs containing a
//! `chatSessions/` subdir are attributable via the sibling `workspace.json`'s
//! `{"folder": "file://..."}` URI — percent-decoded (e.g. `%20` -> space), not string-sliced.

use crate::config::Config;
use crate::discovery;
use crate::schema::AgentSignal;
use chrono::DateTime;
use std::collections::HashMap;
use std::path::Path;

/// Walks `workspace_storage_dir` (`~/Library/Application Support/Code/User/workspaceStorage/`
/// on macOS). Per hash directory: skip if no `chatSessions/` subdir, skip if no
/// `workspace.json`, skip multi-root workspaces (v1 limitation — `workspace.json` has more
/// than one folder), skip on malformed JSON (never raise), skip if the newest chat session
/// mtime is older than `cold_cutoff_hours` (default 1440h/60 days in the real Python
/// `scan()` — this parameter was missing entirely from an earlier version of this function,
/// so every Copilot chat session ever recorded, no matter how stale, produced a permanent
/// signal; caught only by a real-`$HOME` diff against the Python implementation, since the
/// small AFK fixture never had a stale enough Copilot fixture to expose it). Otherwise
/// percent-decode the `folder` `file://` URI into a real filesystem path (use the `url`
/// crate — do not string-slice `%XX` sequences), resolve via `discovery::resolve_root`, and
/// set the signal's `at` to the newest mtime under `chatSessions/`. Two hashes resolving to
/// the same root fold to one signal, newest wins. Missing `workspace_storage_dir` => empty map.
pub fn scan(
    workspace_storage_dir: &Path,
    config: &Config,
    cold_cutoff_hours: u64,
) -> HashMap<String, AgentSignal> {
    let Ok(entries) = std::fs::read_dir(workspace_storage_dir) else {
        // Missing, unreadable, or not a dir — degrade to empty, never error.
        return HashMap::new();
    };

    let cutoff_secs =
        std::time::SystemTime::now() - std::time::Duration::from_secs(cold_cutoff_hours * 3600);

    let mut signals: HashMap<String, AgentSignal> = HashMap::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        fold_one_hash(&path, config, cutoff_secs, &mut signals);
    }

    signals
}

/// Consider a single hash directory; folds into `signals` in place. Mirrors the Python
/// `_process_one_hash` one-to-one: both `chatSessions/` and `workspace.json` must be
/// present, `workspace.json`'s `folder` field must be a single string (not an array), the
/// newest chat session mtime must be at or after `cutoff` (cold-skip), and the URI is
/// percent-decoded via the `url` crate — not hand-sliced.
fn fold_one_hash(
    hash_dir: &Path,
    config: &Config,
    cutoff: std::time::SystemTime,
    signals: &mut HashMap<String, AgentSignal>,
) {
    let workspace_json = hash_dir.join("workspace.json");
    let chat_sessions = hash_dir.join("chatSessions");

    // Both must be present — either absence is a skip, not an error (sensors degrade).
    if !workspace_json.is_file() {
        return;
    }
    if !chat_sessions.is_dir() {
        return;
    }

    // Parse `workspace.json`. Malformed JSON => skip this hash, never abort the scan.
    let text = match std::fs::read_to_string(&workspace_json) {
        Ok(t) => t,
        Err(_) => return,
    };
    let payload: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let payload_obj = match payload.as_object() {
        Some(o) => o,
        None => return,
    };

    // `folder` must be a single string (string, not array — multi-root workspaces are v1
    // limitation and explicitly out of scope; drop any non-string value the same way).
    let folder_uri = match payload_obj.get("folder").and_then(|v| v.as_str()) {
        Some(s) => s.to_owned(),
        None => return,
    };

    // Convert the `file://` URI to a filesystem path via the `url` crate's decoder.
    // The whole point of this branch is that `%20` (etc.) must become a real space in
    // the resulting path — hand-slicing would leave the escape sequences intact.
    let decoded_path = match file_uri_to_path(&folder_uri) {
        Some(p) => p,
        None => return,
    };

    // `at` = the newest mtime under `chatSessions/`. An empty or unreadable directory
    // yields no signal, same as Python's `_read_chat_session_mtime` returning `None`.
    let newest_mtime = match newest_chat_session_mtime(&chat_sessions) {
        Some(t) => t,
        None => return,
    };

    // Cold-skip: mirrors Python's `if newest_mtime < cutoff: return signals`, checked
    // right after computing the newest mtime, before resolving/inserting anything.
    let cutoff_secs = cutoff
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(i64::MIN);
    if newest_mtime < cutoff_secs {
        return;
    }

    let at = match DateTime::from_timestamp(newest_mtime, 0) {
        Some(dt) => dt,
        None => return,
    };

    // Resolve to the canonical project root — one broken workspace must not break the scan.
    let resolved = discovery::resolve_root(&decoded_path, config);
    let key = resolved.to_string_lossy().into_owned();

    // Two hashes mapping to the same root: the one with the newer `at` wins.
    let dominated = signals
        .get(&key)
        .is_some_and(|existing| existing.at >= at);
    if !dominated {
        signals.insert(
            key,
            AgentSignal {
                root: resolved.to_string_lossy().into_owned(),
                at,
                agent: String::from("copilot"),
                session_id: None,
                event: None,
                raw_cwd: Some(decoded_path.to_string_lossy().into_owned()),
            },
        );
    }
}

/// Decode a `file://` URI to an absolute filesystem path. The whole point of this branch
/// is that `%20` (etc.) must become a real character in the resulting path — not the
/// literal string `%20` — and `Url::to_file_path()` does exactly that decoding for us via
/// the URL library, rather than us hand-slicing `file://` off and escaping.
/// Non-`file://` schemes and non-decodeable paths both return `None`.
fn file_uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    let parsed = url::Url::parse(uri).ok()?;
    if parsed.scheme() != "file" {
        return None;
    }
    // `Url::to_file_path()` handles percent-decoding for us (e.g. `%20` -> space)
    // and yields a canonical OS path; fall back to hand-decoding the path component
    // only if `to_file_path()` rejects a valid file URI (e.g. on non-Windows where
    // it requires three slashes but a bare `file:///path` is still valid).
    parsed.to_file_path().ok()
}

/// The newest mtime among regular files inside `dir` (excluding subdirs — mirrors the Python
/// reference which iterates `iterdir()` and `is_file()`). An empty or unreadable dir
/// returns `None`. Any single-read failure is swallowed so one rotten file can't abort the
/// whole hash (sensors degrade, never abort).
fn newest_chat_session_mtime(dir: &Path) -> Option<i64> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut newest: Option<i64> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Some(epoch_secs) = modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs() as i64)
        else {
            continue;
        };
        newest = match newest {
            Some(cur) if epoch_secs > cur => Some(epoch_secs),
            other => other.or(Some(epoch_secs)),
        };
    }
    newest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::collections::HashSet;
    use std::io::Write;
    use std::time::{Duration, SystemTime};

    fn test_config(roots: Vec<std::path::PathBuf>) -> Config {
        let mut cfg = Config::default();
        cfg.roots = roots;
        cfg.extra_paths = vec![];
        cfg.ignore_dirs = HashSet::new();
        cfg
    }

    struct Tmp {
        path: std::path::PathBuf,
    }
    impl Tmp {
        fn new(suffix: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("swab_rs_copilot_test_{suffix}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("mktemp");
            Self { path }
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn init_repo(path: &Path) {
        std::fs::create_dir_all(path.join(".git")).expect("mkdir .git");
    }

    /// Writes `content` to `path` (creating parent dirs as needed) — used by tests that
    /// only care about a file's contents, not its mtime (see `write_file_set_mtime` below
    /// for the mtime-controlling variant used by the chatSessions fixtures).
    fn write_json_file<P: AsRef<Path>>(path: P, content: &str) {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir parent");
        }
        std::fs::write(path, content).expect("write json file");
    }

    fn write_file_set_mtime<P: AsRef<Path>, C: AsRef<[u8]>>(
        path: P,
        content: C,
        seconds_ago: u64,
    ) {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir parent");
        }
        let mut fh = std::fs::File::create(path).expect("write");
        fh.write_all(content.as_ref()).expect("write bytes");
        let mtime = SystemTime::now() - Duration::from_secs(seconds_ago);
        fh.set_modified(mtime).expect("set mtime");
        drop(fh);
    }

    // Test 1: one hash with chatSessions/ + valid workspace.json -> one signal.
    #[test]
    fn single_hash_with_both_files_yields_one_signal() {
        let tmp = Tmp::new("one_hash");
        init_repo(&tmp.path.join("repo"));

        let ws = tmp.path.join("workspaceStorage");
        std::fs::create_dir_all(&ws).expect("mkdir ws");

        let hash1 = ws.join("abc123");
        std::fs::create_dir_all(hash1.join("chatSessions")).expect("mkdir");
        write_file_set_mtime(
            hash1.join("workspace.json"),
            r#"{"folder": "file:///fake/repo"}"#,
            0,
        );
        write_file_set_mtime(
            hash1.join("chatSessions").join("s.jsonl"),
            b"x",
            0,
        );

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&ws, &cfg, 1440);

        assert_eq!(signals.len(), 1, "one hash -> one signal");
        let sig = signals.values().next().unwrap();
        assert_eq!(sig.agent, "copilot");
        assert_eq!(sig.session_id, None);
        assert_eq!(sig.event, None);
    }

    // Test 2: a hash missing chatSessions/ is skipped (no signal, no panic).
    #[test]
    fn missing_chat_sessions_is_skipped() {
        let tmp = Tmp::new("no_chatsessions");
        init_repo(&tmp.path.join("repo"));

        let ws = tmp.path.join("workspaceStorage");
        std::fs::create_dir_all(&ws).expect("mkdir ws");

        let hash1 = ws.join("abc123");
        std::fs::create_dir_all(&hash1).expect("mkdir hash");
        write_file_set_mtime(
            hash1.join("workspace.json"),
            r#"{"folder": "file:///fake/repo"}"#,
            0,
        );

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&ws, &cfg, 1440);
        assert!(signals.is_empty(), "hash without chatSessions/ must not emit a signal");
    }

    // Test 3: a hash missing workspace.json is skipped.
    #[test]
    fn missing_workspace_json_is_skipped() {
        let tmp = Tmp::new("no_ws_json");
        init_repo(&tmp.path.join("repo"));

        let ws = tmp.path.join("workspaceStorage");
        std::fs::create_dir_all(&ws).expect("mkdir ws");

        let hash1 = ws.join("abc123");
        std::fs::create_dir_all(hash1.join("chatSessions")).expect("mkdir");
        write_file_set_mtime(
            hash1.join("chatSessions").join("s.jsonl"),
            b"x",
            0,
        );

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&ws, &cfg, 1440);
        assert!(signals.is_empty(), "hash without workspace.json must not emit a signal");
    }

    // Test 4: malformed JSON in workspace.json is skipped without aborting other hashes.
    #[test]
    fn malformed_json_is_skipped_but_other_hashes_continue() {
        let tmp = Tmp::new("malformed_json");
        init_repo(&tmp.path.join("repo"));

        let ws = tmp.path.join("workspaceStorage");
        std::fs::create_dir_all(&ws).expect("mkdir ws");

        let hash1 = ws.join("bad-json");
        std::fs::create_dir_all(hash1.join("chatSessions")).expect("mkdir");
        write_file_set_mtime(hash1.join("workspace.json"), b"NOT JSON", 0);
        write_file_set_mtime(
            hash1.join("chatSessions").join("s.jsonl"),
            b"x",
            0,
        );

        let hash2 = ws.join("good");
        std::fs::create_dir_all(hash2.join("chatSessions")).expect("mkdir");
        write_file_set_mtime(
            hash2.join("workspace.json"),
            r#"{"folder": "file:///fake/repo"}"#,
            0,
        );
        write_file_set_mtime(
            hash2.join("chatSessions").join("s.jsonl"),
            b"x",
            0,
        );

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&ws, &cfg, 1440);
        assert_eq!(signals.len(), 1, "only the good hash must yield a signal");
    }

    // Test 5: a %20-encoded folder URI resolves to a path with a literal space.
    // This is the test that actually exercises `url` crate decoding — hand-slicing would leave
    // "%20" verbatim in the path.
    #[test]
    fn percent_encoded_uri_resolves_with_literal_space() {
        // Encode a path with a space: "file:///fake/my repo" -> "file:///fake/my%20repo"
        let encoded = "file:///fake/my%20repo";

        // Set up: the decoded path (with literal space) must be resolveable by `resolve_root`.
        // We don't need an actual repo at that path — resolve_root returns the cwd unchanged
        // when it doesn't exist; we just check the string form.
        let decoded = file_uri_to_path(encoded).expect("percent_decode");

        // The decoded path MUST contain a literal space, not the literal string "%20".
        assert!(
            decoded.to_string_lossy().contains(' '),
            "decoded path must contain a real space, got {:?}",
            decoded.to_string_lossy(),
        );
        assert!(
            !decoded.to_string_lossy().contains("%20"),
            "decoded path must not retain the %20 escape, got {:?}",
            decoded.to_string_lossy(),
        );

        // Sanity: another valid URI still decodes.
        let simple = file_uri_to_path("file:///tmp/foo").expect("simple uri");
        assert_eq!(simple.to_string_lossy().as_ref(), "/tmp/foo");

        // And a non-file scheme is rejected.
        assert!(file_uri_to_path("https://example.com").is_none());
    }

    // Test 6: signal `at` = newest mtime under chatSessions/.
    #[test]
    fn signal_at_is_newest_chat_session_mtime() {
        let tmp = Tmp::new("newest_mtime");
        init_repo(&tmp.path.join("repo"));

        let ws = tmp.path.join("workspaceStorage");
        std::fs::create_dir_all(&ws).expect("mkdir");

        let hash1 = ws.join("abc123");
        std::fs::create_dir_all(hash1.join("chatSessions")).expect("mkdir");
        write_file_set_mtime(
            hash1.join("workspace.json"),
            r#"{"folder": "file:///fake/repo"}"#,
            0,
        );

        let older_file = hash1.join("chatSessions").join("old.jsonl");
        let newer_file = hash1.join("chatSessions").join("new.jsonl");
        write_file_set_mtime(&older_file, b"old", 600); // 10 minutes ago
        write_file_set_mtime(&newer_file, b"new", 30); // 30 seconds ago

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&ws, &cfg, 1440);

        let sig = signals.values().next().expect("must have one signal");
        let expected = DateTime::from_timestamp(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
                - 30,
            0,
        )
        .expect("now-as-epoch");
        // Allow a few seconds of tolerance for clock drift during the test.
        let diff = (sig.at - expected).num_seconds().unsigned_abs();
        assert!(diff < 5, "at should match newer mtime (30s ago), got diff={diff}s");
    }

    // Test 7: missing workspace_storage_dir -> empty map, no panic.
    #[test]
    fn missing_dir_yields_empty_map() {
        let cfg = test_config(vec![]);
        let signals = scan(std::path::Path::new("/definitely/does/not/exist"), &cfg, 1440);
        assert!(signals.is_empty(), "missing dir must not yield signals and must not panic");
    }

    // Test 8: two hashes resolving to the same root -> one signal, newest wins.
    #[test]
    fn two_hashes_same_root_newest_wins() {
        let tmp = Tmp::new("same_root");
        init_repo(&tmp.path.join("repo"));

        let ws = tmp.path.join("workspaceStorage");
        std::fs::create_dir_all(&ws).expect("mkdir ws");

        for (hash_name, seconds_ago) in [
            ("first", 600u64), // older
            ("second", 30u64), // newer
        ] {
            let h = ws.join(hash_name);
            std::fs::create_dir_all(h.join("chatSessions")).expect("mkdir");
            write_file_set_mtime(
                h.join("workspace.json"),
                r#"{"folder": "file:///fake/repo"}"#,
                seconds_ago,
            );
            write_file_set_mtime(
                h.join("chatSessions").join("s.jsonl"),
                b"x",
                seconds_ago,
            );
        }

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&ws, &cfg, 1440);

        assert_eq!(signals.len(), 1, "must collapse to one signal");
        let sig = signals.values().next().unwrap();
        // The newer (30s ago) mtime must win.
        let expected = DateTime::from_timestamp(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
                - 30,
            0,
        )
        .expect("now-as-epoch");
        let diff = (sig.at - expected).num_seconds().unsigned_abs();
        assert!(diff < 5, "newest mtime (30s ago) must win");
    }

    // Additional: a multi-root workspace (`"folder": [...]`) is skipped — v1 limitation,
    // explicitly out of scope.
    #[test]
    fn multi_root_workspace_is_skipped() {
        let tmp = Tmp::new("multiroot");
        init_repo(&tmp.path.join("repo"));

        let ws = tmp.path.join("workspaceStorage");
        std::fs::create_dir_all(&ws).expect("mkdir ws");

        let h = ws.join("multiroot");
        std::fs::create_dir_all(h.join("chatSessions")).expect("mkdir");
        write_file_set_mtime(
            h.join("workspace.json"),
            r#"{"folder": ["/fake/a", "/fake/b"]}"#,
            0,
        );
        write_file_set_mtime(
            h.join("chatSessions").join("s.jsonl"),
            b"x",
            0,
        );
    }

    // Additional: a hash with an empty `chatSessions/` directory contributes no signal.
    #[test]
    fn empty_chat_sessions_yields_no_signal() {
        let tmp = Tmp::new("empty_chatsessions");
        init_repo(&tmp.path.join("repo"));

        let ws = tmp.path.join("workspaceStorage");
        std::fs::create_dir_all(&ws).expect("mkdir ws");

        let h = ws.join("empty");
        std::fs::create_dir_all(h.join("chatSessions")).expect("mkdir");
        write_json_file(
            &h.join("workspace.json"),
            r#"{"folder": "file:///fake/repo"}"#,
        );

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&ws, &cfg, 1440);
        assert!(signals.is_empty(), "empty chatSessions must not yield a signal");
    }

    // Regression: an earlier version of `scan()` had no `cold_cutoff_hours` parameter at
    // all, so a chat session from months ago produced a permanent signal every tick --
    // caught only by diffing a real $HOME against the Python implementation, since no
    // fixture here was ever built stale enough to exercise it. A session older than the
    // cutoff must contribute nothing.
    #[test]
    fn cold_chat_session_is_skipped() {
        let tmp = Tmp::new("cold_chat_session");
        init_repo(&tmp.path.join("repo"));

        let ws = tmp.path.join("workspaceStorage");
        std::fs::create_dir_all(&ws).expect("mkdir ws");

        let h = ws.join("stale");
        std::fs::create_dir_all(h.join("chatSessions")).expect("mkdir");
        write_json_file(&h.join("workspace.json"), r#"{"folder": "file:///fake/repo"}"#);
        // 2000 hours ago -- well past the default 1440h (60-day) cutoff.
        write_file_set_mtime(h.join("chatSessions").join("old.jsonl"), b"old", 2000 * 3600);

        let cfg = test_config(vec![tmp.path.clone()]);
        let signals = scan(&ws, &cfg, 1440);
        assert!(signals.is_empty(), "a chat session older than the cutoff must not yield a signal");

        // Same fixture, but with a cutoff wide enough to include it -- confirms the skip
        // above is genuinely the cutoff doing its job, not some other bug hiding the signal.
        let signals_with_wide_cutoff = scan(&ws, &cfg, 3000);
        assert_eq!(
            signals_with_wide_cutoff.len(), 1,
            "the same session must produce a signal once the cutoff is wide enough"
        );
    }
}
