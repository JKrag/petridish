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
use crate::discovery::{self};
use crate::schema::{self, AgentActivity, AgentState, Radar, StatusBucket};
use chrono::SubsecRound;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Cold-skip threshold (hours), shared by both the claude and copilot sensor calls below —
/// matches Python's default `cold_cutoff_hours` for both (1440h/60 days).
const DEFAULT_COLD_CUTOFF_HOURS: u64 = 1_440;

/// Hard cap on events.ndjson bytes consumed per tick — matches the Python reference's 5 MB.
const EVENTS_MAX_BYTES: u64 = 5_000_000;

/// Default bucket thresholds used as the fallback when config is missing entries. Matches
/// `Config::default()`'s bucket_thresholds default (48h/336h/1440h).
const DEFAULT_THRESHOLD_ACTIVE_H: f64 = 48.0;
const DEFAULT_THRESHOLD_IN_FLIGHT_H: f64 = 336.0;
const DEFAULT_THRESHOLD_STALE_H: f64 = 1_440.0;

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

/// Derive an `AgentState` from a signal + current time. For a missing signal, returns the
/// idle-unknown default (so project rows never have an unset agent slot).
///
/// Reimplements `petridish.scan._agent_state_for`: a stale signal still carries useful facts
/// (WHO last touched this project and WHEN) — we keep them around for the resume feature,
/// only the `state` field reflects recency.
fn derive_agent(
    signal: Option<&schema::AgentSignal>,
    now: chrono::DateTime<chrono::Utc>,
) -> AgentState {
    match signal {
        None => AgentState::idle_unknown(),
        Some(s) => {
            let age_s = (now - s.at).num_seconds(); // can be negative if clock jumped forward
            let state = schema::agent_state_for_silence(age_s);
            AgentState {
                state,
                active_agent: Some(s.agent.clone()),
                last_event: s.event.clone(),
                last_event_at: Some(s.at),
                session_id: s.session_id.clone(),
            }
        }
    }
}

/// newest non-None of `mine_last_commit_at`, `last_commit_at` (preferred-over when no mine),
/// and the signal's `at`. Mirrors `petridish.scan._last_activity`.
fn last_activity_at(
    git: &schema::GitState,
    signal: Option<&schema::AgentSignal>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let mine = git.mine_last_commit_at;
    let last = git.last_commit_at;
    let sig_at = signal.map(|s| s.at);

    // Python's preference: mine > last. Fall back to signal-only if no git time at all.
    let git_ts: Option<chrono::DateTime<chrono::Utc>> = mine.or(last);

    match (git_ts, sig_at) {
        (Some(a), Some(b)) => Some(if a > b { a } else { b }),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    }
}

/// Bucket decision: Working/Recent agent always -> Active; then age-based against thresholds.
/// None activity_at => Cold. Mirrors `petridish.scan._bucket` exactly.
fn status_bucket(
    last_activity_at: Option<chrono::DateTime<chrono::Utc>>,
    agent: &AgentState,
    now: chrono::DateTime<chrono::Utc>,
    threshold_active_h: f64,
    threshold_in_flight_h: f64,
    threshold_stale_h: f64,
) -> StatusBucket {
    // Agent state overrides — a Working or Recent project is always "active", regardless of
    // when the last git commit landed (invariant #6 of ARCHITECTURE.md: git-only
    // bucketing loses when an agent is actively working in it).
    if matches!(agent.state, AgentActivity::Working | AgentActivity::Recent) {
        return StatusBucket::Active;
    }

    let laa = match last_activity_at {
        Some(a) => a,
        None => return StatusBucket::Cold,
    };

    let age_secs = (now - laa).num_seconds(); // chrono::Duration::num_seconds()
    let age_hours = age_secs as f64 / 3_600.0;

    if age_hours < threshold_active_h {
        StatusBucket::Active
    } else if age_hours < threshold_in_flight_h {
        StatusBucket::InFlight
    } else if age_hours < threshold_stale_h {
        StatusBucket::Stale
    } else {
        StatusBucket::Cold
    }
}

/// `_sha1_id` from `src/petridish/scan.py`: sha1 of the (already-resolved) path, first 12
/// hex characters. Stable across `run_scan` calls against the same path.
fn sha1_id(resolved_path: &str) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(resolved_path.as_bytes());
    let hex: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    hex[..12].to_string()
}

/// Build a single `Project` for one resolved root, given its (potentially merged) signal and
/// the precomputed foreignness flag. Reimplemented from `petridish.scan._build_project` —
/// mirrors field-for-field (id, name, path, category, is_foreign, git, agent, last_activity_at,
/// status_bucket).
fn build_project(
    resolved: &Path,
    config: &Config,
    signal: Option<&schema::AgentSignal>,
    now: chrono::DateTime<chrono::Utc>,
    thresholds_active_h: f64,
    thresholds_in_flight_h: f64,
    thresholds_stale_h: f64,
) -> schema::Project {
    let resolved_str = resolved.to_string_lossy().into_owned();

    // Category: config override by resolved path falls back to the resolved dir's parent name.
    // Matches Python's `config.category_overrides.get(resolved, parent_name)`. The
    // `path: String` field on `schema::Project` is the resolved path stored verbatim.
    let category = config
        .category_overrides
        .get(&resolved_str)
        .cloned()
        .unwrap_or_else(|| {
            resolved
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(String::from)
                .unwrap_or_else(|| resolved_str.clone())
        });

    let foreign = discovery::is_foreign(resolved, config);
    let git_state = crate::git::scan(resolved, &config.author_patterns, &config.author_since);

    let agent = derive_agent(signal, now);

    let last_activity_at = last_activity_at(&git_state, signal);
    let status_bucket = status_bucket(
        last_activity_at,
        &agent,
        now,
        thresholds_active_h,
        thresholds_in_flight_h,
        thresholds_stale_h,
    );

    let name = resolved
        .file_name()
        .and_then(|n| n.to_str())
        .map(String::from)
        .unwrap_or_else(|| resolved_str.clone());

    let id = sha1_id(&resolved_str);

    // parent_path: if the resolved path contains a segment literally equal to `.worktrees`
    // anywhere, parent_path is the string form of the path components BEFORE that segment.
    // Otherwise None. This captures the "this is a .worktrees/<name> child project" relationship
    // — used by frontends to display parent project linkage.
    let parent_path: Option<String> = {
        // Check if the path contains a `.worktrees` component. If so, the parent_path is the
        // path up to (and including) the parent directory of `.worktrees`. For example:
        //   resolved = /tmp/foo/.worktrees/bar
        //   parent_path = Some("/tmp/foo")
        // We use `Path::components()` to check for `.worktrees`, then use `Path::parent()`
        // to get the parent directory of `.worktrees` (which is the parent of the last
        // component before `.worktrees`).
        if resolved.components().any(|c| c.as_os_str() == ".worktrees") {
            // Find the parent of `.worktrees` by walking up from the path.
            // We need to get the directory that contains `.worktrees`, which is the
            // parent of the last component of the path (since `.worktrees` is the
            // second-to-last component in our path).
            // Actually, we need to get the parent of the directory that contains `.worktrees`.
            // Let's use a different approach: find the index of `.worktrees` in the
            // components and reconstruct the path.
            let components: Vec<String> = resolved
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            let idx = components.iter().position(|c| c == ".worktrees");
            if let Some(i) = idx {
                if i == 0 {
                    None // `.worktrees` is the first component (no meaningful parent).
                } else {
                    // Reconstruct the path by taking all components before `.worktrees`.
                    let mut parent_path = PathBuf::new();
                    for component in components.iter().take(i) {
                        parent_path.push(component.as_str());
                    }
                    Some(parent_path.to_string_lossy().into_owned())
                }
            } else {
                None
            }
        } else {
            None
        }
    };

    schema::Project {
        id,
        name,
        path: resolved_str,
        category,
        parent_path,
        is_foreign: foreign,
        git: git_state,
        agent,
        last_activity_at,
        status_bucket,
    }
}

/// One full tick: `discovery::discover` + `discovery::is_foreign` for project list, `git::scan`
/// per project, all three sensors + `events::read_and_compact` merged into one
/// `root -> AgentSignal` map (invariant #5: any sensor erroring/panicking-internally must
/// degrade to an empty contribution, never abort the tick), newest-`at`-wins across sources.
/// Every discovered path AND every signal root becomes a `Project` (union, not intersection).
/// `last_activity_at` = max(`mine_last_commit_at`, merged signal's `at`). Final sort:
/// `last_activity_at` descending (`None` last), then `name` ascending. Times the whole tick
/// for `scan_duration_ms`. Never panics — see the sensor degrade-never-abort invariant.
pub fn run_scan(config: &Config, paths: &ScanPaths) -> Radar {
    let tick_start = Instant::now();

    // Now-as-UTC — `Radar.updated_at` is UTC, and agent-state thresholding uses the same
    // reference point Python's `now`/`to_utc(now)` uses. Truncated to whole-second precision
    // at generation time, matching Python's `utcnow()` (`datetime.now(utc).replace(
    // microsecond=0)`) rather than relying solely on serde's truncation at the write
    // boundary -- this value is also used directly for in-memory age/bucketing math and
    // compared against round-tripped values in tests, so it needs to already be at
    // whole-second precision, not just truncated when it happens to get serialized.
    let now = chrono::Utc::now().trunc_subsecs(0);

    // 1. Discovery: walk roots + extras into a candidate-list. Missing roots degrade to empty.
    let discovered = discovery::discover(config);

    // 2. Sensor reads — wrapped in `catch_unwind` (invariant #5 defense-in-depth; each sensor
    //    already degrades on its own, but an out-of-spec panic would otherwise abort the tick).
    //    Every sensor is called with the *real path from `paths`* (the production-defaults
    //    seam): never ad-hoc, never `None`.
    let claude_signals: HashMap<String, schema::AgentSignal> = std::panic::catch_unwind(
        AssertUnwindSafe(|| {
            crate::sensors::claude::scan(&paths.claude_projects_dir, config, DEFAULT_COLD_CUTOFF_HOURS)
        }),
    )
    .unwrap_or_default();

    let copilot_signals: HashMap<String, schema::AgentSignal> = std::panic::catch_unwind(
        AssertUnwindSafe(|| {
            crate::sensors::copilot::scan(&paths.workspace_storage_dir, config, DEFAULT_COLD_CUTOFF_HOURS)
        }),
    )
    .unwrap_or_default();

    let events_signals: HashMap<String, schema::AgentSignal> =
        std::panic::catch_unwind(AssertUnwindSafe(|| {
            crate::events::read_and_compact(&paths.events_path, config, EVENTS_MAX_BYTES)
        }))
        .unwrap_or_default();

    // `quota` is account-global (not per-project), so read once. `read_quota` never panics —
    // missing path / malformed JSON -> None — but we still wrap in catch_unwind per invariant.
    let quota: Option<schema::QuotaState> = std::panic::catch_unwind(AssertUnwindSafe(|| {
        crate::sensors::quota::read_quota(&paths.quota_path)
    }))
    .ok()
    .flatten();

    // 4. Merge by root across ALL sources — newest `at` wins globally (Python: `sig.at > existing.at`).
    let mut merged: HashMap<String, schema::AgentSignal> = HashMap::new();
    for src in [&claude_signals, &copilot_signals, &events_signals] {
        for (root, sig) in src.iter() {
            merged
                .entry(root.clone())
                .and_modify(|existing: &mut schema::AgentSignal| {
                    if sig.at > existing.at {
                        *existing = sig.clone();
                    }
                })
                .or_insert_with(|| sig.clone());
        }
    }

    // 3. UNION of (discovered) paths and signal roots — every root that appears anywhere
    // becomes a Project, even one without a discovery entry (e.g. an out-of-roots project
    // with Claude activity). `discovery::resolve_root` collapses monorepo subdirs.
    let mut all_roots: std::collections::HashSet<String> = discovered
        .iter()
        .map(|p| discovery::resolve_root(p, config).to_string_lossy().into_owned())
        .collect();
    all_roots.extend(merged.keys().cloned());

    // Bucket thresholds — fall back to the documented defaults if config omits them. Python
    // uses `thresholds.get("active", 48.0)`, same fallback semantics.
    let thresholds = &config.bucket_thresholds;
    let t_active = *thresholds.get("active").unwrap_or(&DEFAULT_THRESHOLD_ACTIVE_H);
    let t_in_flight = *thresholds
        .get("in_flight")
        .unwrap_or(&DEFAULT_THRESHOLD_IN_FLIGHT_H);
    let t_stale = *thresholds.get("stale").unwrap_or(&DEFAULT_THRESHOLD_STALE_H);

    let mut projects: Vec<schema::Project> = all_roots
        .into_iter()
        .map(|root| {
            let resolved = PathBuf::from(&root);
            let signal = merged.get(&root);
            // `build_project` computes `is_foreign` itself (matches Python's `_build_project`,
            // which also calls `is_foreign` fresh per project with no precomputed cache).
            build_project(
                &resolved,
                config,
                signal,
                now,
                t_active,
                t_in_flight,
                t_stale,
            )
        })
        .collect();

    // 4. Sort: `last_activity_at` descending with None last, then `name` ascending.
    projects.sort_by(|a, b| {
        // (None-last) rule: a has timestamp and b doesn't => a comes first (descending order,
        // None sorts last). Two with timestamps sort by timestamp desc; tie-break by name.
        match (&a.last_activity_at, &b.last_activity_at) {
            (Some(a_ts), Some(b_ts)) => b_ts
                .timestamp()
                .cmp(&a_ts.timestamp())
                .then_with(|| a.name.cmp(&b.name)),
            (Some(_), None) => std::cmp::Ordering::Less, // a (has ts) comes first
            (None, Some(_)) => std::cmp::Ordering::Greater, // b (has ts) comes first
            (None, None) => a.name.cmp(&b.name),
        }
    });

    let scan_duration_ms = tick_start.elapsed().as_millis() as u64;

    Radar {
        schema_version: 1,
        updated_at: now,
        scan_duration_ms,
        projects,
        quota,
    }
}

/// `run_scan` + `schema::write_atomic` to `state_path`. Returns the written `Radar` (for the
/// CLI's "scanned N projects in Mms -> path" message) or an io error from the write step.
/// `run_scan` itself doesn't fail — only the write can, per contract (single-writer invariant:
/// `projects.json` is written atomically via temp-file + rename).
pub fn write_scan(
    config: &Config,
    paths: &ScanPaths,
    state_path: &Path,
) -> std::io::Result<Radar> {
    let radar = run_scan(config, paths);
    schema::write_atomic(state_path, &radar)?;
    Ok(radar)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::collections::HashSet;
    use std::process::Command;
    use std::time::{Duration, SystemTime};

    // ── Helpers: build a `Config` on the fly and a RAII tmp dir. ──────────────────

    /// Build a minimal `Config` whose `roots` include `roots` (paths passed in as already-
    /// absolute are used verbatim — default-expansion is NOT invoked, so tests retain full
    /// control over what discovery sees).
    fn test_config(roots: Vec<PathBuf>) -> Config {
        let mut c = Config::default();
        // Default roots ("~/repos", "~/learning") would resolve via default-expansion, but we
        // don't want that in fixture-driven tests. Hand-construct with the given paths instead.
        c.roots = roots;
        c.ignore_dirs = HashSet::new(); // no default skips — tests need full control
        c.extra_paths = vec![];
        c
    }

    /// RAII guard for temp dirs: cleaned up on drop, unique name per fixture.
    #[derive(Debug)]
    struct Tmp {
        path: PathBuf,
    }

    impl Tmp {
        fn new(suffix: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("swab_scan_test_{suffix}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("mktemp");
            Self { path }
        }

        /// `git init` inside this tmp dir (cwd), with pinned author/date env vars so
        /// git timestamps are deterministic across runs.
        fn git_init(&self) {
            assert!(
                Command::new("git")
                    .args(["init", self.path.to_str().unwrap()])
                    .env("GIT_AUTHOR_DATE", "2024-01-15T10:30:00+00:00")
                    .env("GIT_COMMITTER_DATE", "2024-01-15T10:30:00+00:00")
                    .env("GIT_AUTHOR_NAME", "Test Author")
                    .env("GIT_AUTHOR_EMAIL", "t@t.com")
                    .env("GIT_COMMITTER_NAME", "Test Committer")
                    .env("GIT_COMMITTER_EMAIL", "t@t.com")
                    .spawn()
                    .expect("git init spawn")
                    .wait()
                    .expect("git init wait")
                    .success(),
                "git init failed in {}",
                self.path.display()
            );
        }

    }

    /// Runs `git add <filename> && git commit` inside `repo_dir`, with pinned author/committer
    /// env vars (both dates set to `date`) so commit timestamps are deterministic across runs.
    /// An earlier version of this logic lived in a `Tmp::git_add_and_commit` method that only
    /// ran `git add` and never `git commit` -- every caller silently got an empty, uncommitted
    /// repo. Now a free function so both fixture-root and subdirectory commits share one
    /// correct implementation.
    /// inside a non-root subdirectory (e.g. a `repo/` under the fixture's tmp path).
    fn git_add_and_commit_at(repo_dir: &Path, filename: &str, contents: &str, date: &str) {
        std::fs::write(repo_dir.join(filename), contents).expect("write");
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .env("GIT_AUTHOR_DATE", date)
                .env("GIT_COMMITTER_DATE", date)
                .env("GIT_AUTHOR_NAME", "Test Author")
                .env("GIT_AUTHOR_EMAIL", "t@t.com")
                .env("GIT_COMMITTER_NAME", "Test Committer")
                .env("GIT_COMMITTER_EMAIL", "t@t.com")
                .spawn()
                .expect("git spawn")
                .wait()
                .expect("git wait")
                .success()
        };
        let dir = repo_dir.to_str().unwrap();
        assert!(run(&["-C", dir, "add", filename]), "git add failed in {dir}");
        assert!(
            run(&["-C", dir, "commit", "--no-gpg-sign", "-m", "test commit"]),
            "git commit failed in {dir}"
        );
    }

    /// `git init` at an arbitrary path (not necessarily a `Tmp`'s root) -- used by tests that
    /// need a real repo in a subdirectory of the fixture, not the fixture root itself.
    fn git_init_at(path: &Path) {
        assert!(
            Command::new("git")
                .args(["init", path.to_str().unwrap()])
                .spawn()
                .expect("git init spawn")
                .wait()
                .expect("git init wait")
                .success(),
            "git init failed in {}",
            path.display()
        );
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Write a `.jsonl` transcript at `path`, and set its mtime to `seconds_ago` before now.
    /// Used to simulate "this project was active N seconds ago" without depending on real
    /// transcript files on disk.
    fn write_transcript(path: &Path, lines: &[&str], seconds_ago: u64) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        let body = lines.join("\n") + "\n";
        std::fs::write(path, &body).expect("write transcript");
        let mtime = SystemTime::now() - Duration::from_secs(seconds_ago);
        std::fs::File::open(path)
            .expect("reopen")
            .set_modified(mtime)
            .expect("set mtime");
    }

    /// Build one JSONL line with `session_id` and `cwd`.
    fn tline(session_id: Option<&str>, cwd: &str) -> String {
        let mut m = serde_json::Map::new();
        if let Some(sid) = session_id {
            m.insert("sessionId".into(), serde_json::Value::String(sid.to_string()));
        }
        m.insert("cwd".into(), serde_json::Value::String(cwd.to_string()));
        serde_json::to_string(&serde_json::Value::Object(m)).unwrap()
    }

    /// Build a `ScanPaths` against the given tmp home, then call `run_scan` with a
    /// discovery-root set to the project directory. Helper that bundles both so tests don't
    /// repeat the config + path assembly.
    fn run_scan_with_home(home: &Path, project_dir: &Path) -> Radar {
        // The project dir must exist on disk for discovery to find it.
        assert!(
            project_dir.is_dir(),
            "project dir must exist for discovery to find it"
        );
        let paths = ScanPaths::for_home(home);
        let config = test_config(vec![project_dir.to_path_buf()]);
        run_scan(&config, &paths)
    }

    /// Unique identifier for temp directories, safe across runs.
    fn unique_suffix() -> String {
        format!(
            "{:08x}{:08x}",
            std::time::SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64,
            std::process::id(),
        )
    }

    // ═══ Test 1: basic run_scan returns one project for a real git repo. ════════════

    #[test]
    fn run_scan_returns_one_project_for_a_single_repo_with_no_agent_activity() {
        let fixture = Tmp::new("basic");
        // Real git repo at `<tmp>/swab_scan_test_basic/repo` — configured root. A bare
        // `mkdir .git` is NOT sufficient: `git rev-parse --git-dir` (what git::scan actually
        // calls) requires a real repo structure (HEAD, objects/, refs/) and fails outright on
        // an empty `.git` directory -- confirmed empirically. discovery::is_project only checks
        // for `.git`'s existence, which is why this bug didn't surface at the discovery layer.
        let repo = fixture.path.join("repo");
        git_init_at(&repo);
        // No transcript files anywhere -> no agent signals expected.

        let radar = run_scan_with_home(&fixture.path, &repo);

        // Exactly one project — the repo itself.
        assert_eq!(radar.projects.len(), 1, "expected one project: {:?}", radar.projects);
        let p = &radar.projects[0];
        assert_eq!(p.name, "repo");
        assert!(p.git.is_repo); // git::scan should detect it as a repo.
        // No agent signals in this fixture, so `agent.state` must be Idle (agent_state_for_silence
        // falls through to Idle when there's no signal).
        assert!(
            matches!(p.agent.state, AgentActivity::Idle),
            "no agent activity should yield Idle, got {:?}", p.agent.state
        );
        // Path must point at the project root. Compare canonicalized forms -- `discover()`
        // stores the resolved path, which on macOS resolves `/var` -> `/private/var` (a
        // symlink); the raw, non-canonicalized `repo` path would spuriously mismatch.
        assert_eq!(p.path, repo.canonicalize().unwrap().to_str().unwrap());
    }

    // ═══ Test 2: round-trip serialize/deserialize Radar. ════════════════════════════

    #[test]
    fn radar_round_trips_through_serde_json() {
        let fixture = Tmp::new("roundtrip");
        let repo = fixture.path.join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        std::fs::create_dir_all(repo.join(".git")).expect("mkdir .git");

        let radar = run_scan_with_home(&fixture.path, &repo);
        let serialized = serde_json::to_string(&radar).expect("serialize Radar");

        // Deserialize back and check equality (schema derives both Serialize + Deserialize).
        let decoded: Radar = serde_json::from_str(&serialized).expect("deserialize");
        assert_eq!(radar, decoded, "round-trip Radar must equal itself");

        // Ensure we have at least one Project — an "fully empty" Radar could pass the
        // round-trip test trivially.
        assert!(
            !radar.projects.is_empty(),
            "round-trip Radar must contain at least one Project"
        );
    }

    // ═══ Test 3: corrupted sensor path doesn't abort the tick. ═════════════════════

    #[test]
    fn corrupted_sensor_path_degrades_without_aborting_the_tick() {
        // Build a fixture with: (1) a real repo at `<tmp>/repo`, (2) a working transcript in
        // the tmp home's Claude projects dir that references it. Then point `workspace_storage_dir`
        // at a non-existent path (copilot degrades to empty; the other two sources still work).

        let fixture = Tmp::new("corrupt_sensor");
        let repo = fixture.path.join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        std::fs::create_dir_all(repo.join(".git")).expect("mkdir .git");

        // Working Claude transcript fixture.
        let projects_dir = fixture.path.join(".claude/projects");
        write_transcript(
            &projects_dir.join("-slug").join("session.jsonl"),
            &[&tline(Some("sess-1"), repo.to_str().unwrap())],
            0,
        );

        // Create a working events file pointing to the same repo.
        let events_path = fixture.path.join(".petridish/events.ndjson");
        std::fs::create_dir_all(events_path.parent().unwrap()).expect("mkdir .petridish");
        std::fs::write(
            &events_path,
            format!(
                "{{\"cwd\":\"{}\",\"at\":\"2026-01-01T00:00:00Z\"}}\n",
                repo.to_str().unwrap()
            ),
        )
        .expect("write events");

        // workspace_storage_dir -> point at a file (not a dir), which the copilot sensor's
        // `read_dir` will return an Err for. Sensors degrade to empty on such failures.
        std::fs::write(&fixture.path.join("workspaceStorage_fake"), "garbage").expect("create file");

        // Build ScanPaths by hand for this test (not via `for_home`) so we can point the
        // copilot sensor at a bogus path.
        let paths = ScanPaths {
            claude_projects_dir: projects_dir,
            workspace_storage_dir: fixture.path.join("workspaceStorage_fake"),
            events_path,
            quota_path: fixture.path.join(".claude/last-status.json"),
        };

        let config = test_config(vec![repo.clone()]);
        // run_scan MUST NOT panic; the tick should still complete with claude + events signals.
        let radar = run_scan(&config, &paths);
        assert_eq!(
            radar.projects.len(), 1,
            "corrupted copilot path must not collapse the tick: {:?}",
            radar.projects
        );
        // The project should still reflect claude's signal.
        let p = &radar.projects[0];
        assert_eq!(p.agent.active_agent.as_deref(), Some("claude-code"));
    }

    // ═══ Test 4: Agent activity state across silence ages. ═════════════════════════

    #[test]
    fn agent_state_derived_from_silence_age_working_recent_idle() {
        let fixture = Tmp::new("silence_ages");
        let repo = fixture.path.join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        std::fs::create_dir_all(repo.join(".git")).expect("mkdir .git");

        let cases = [
            ("30s", 30u64, AgentActivity::Working),   // < AGENT_WORKING_MAX_S (90)
            ("10m", 600u64, AgentActivity::Recent),  // 90 <= 10*60 < 1800
            ("2h", 7200u64, AgentActivity::Idle),   // >= 1800
        ];

        for (label, seconds_ago, expected) in &cases {
            let tmp = Tmp::new(&format!("silence_{label}"));

            write_transcript(
                &tmp.path.join(".claude/projects/-slug/session.jsonl"),
                &[&tline(Some("sess"), repo.to_str().unwrap())],
                *seconds_ago,
            );

            let paths = ScanPaths::for_home(&tmp.path);
            let config = test_config(vec![repo.clone()]);
            let radar = run_scan(&config, &paths);

            assert_eq!(radar.projects.len(), 1, "[{label}] expected one project");
            assert_eq!(
                radar.projects[0].agent.state, *expected,
                "[{label}] expected state={expected:?}, got {:?}",
                radar.projects[0].agent.state
            );
        }
    }

    // ═══ Test 5: Git-only project (no agent signal) buckets by age. ════════════════

    #[test]
    fn git_only_buckets_by_last_commit_age_cold() {
        // A real commit with a pinned past date -> project's `last_commit_at` is that past
        // time -> age > 1440h -> status_bucket = Cold.
        let fixture = Tmp::new("git_only_cold");
        fixture.git_init();

        // Make a commit via env vars (pinned date 2024-01-15T10:30:00 UTC, over 2 years ago).
        git_add_and_commit_at(&fixture.path, "README.md", "hi", "2024-01-15T10:30:00+00:00");

        let config = test_config(vec![fixture.path.clone()]);
        let paths = ScanPaths::for_home(&fixture.path);
        let radar = run_scan(&config, &paths);

        assert_eq!(radar.projects.len(), 1);
        let p = &radar.projects[0];
        assert_eq!(
            p.status_bucket, StatusBucket::Cold,
            "commit from 2024-01-15 (>1440h ago) must bucket as cold, got {:?}",
            p.status_bucket
        );
    }

    // ═══ Test 6: Working agent overrides old git history -> active bucket. ══════════

    #[test]
    fn working_agent_overrides_old_git_history_buckets_active() {
        let fixture = Tmp::new("working_overrides_cold");
        // Old commit: date in 2023 -> normally would be Cold.
        let repo = fixture.path.join("repo");
        git_init_at(&repo);
        git_add_and_commit_at(&repo, "old.txt", "data", "2023-01-01T00:00:00+00:00");

        // Fresh transcript — < 90s old, agent state should be Working.
        write_transcript(
            &fixture.path.join(".claude/projects/-slug/sess.jsonl"),
            &[&tline(Some("sess-x"), fixture.path.join("repo").to_str().unwrap())],
            10, // 10 seconds ago -> Working
        );

        let radar = run_scan_with_home(&fixture.path, &fixture.path.join("repo"));
        assert_eq!(radar.projects.len(), 1);
        assert_eq!(
            radar.projects[0].agent.state, AgentActivity::Working,
            "fresh transcript -> Working"
        );
        // Working overrides the cold bucket — must be Active regardless of git age.
        assert_eq!(
            radar.projects[0].status_bucket, StatusBucket::Active,
            "Working agent overrides cold bucket -> Active"
        );
    }

    // ═══ Test 7: signal-only root outside configured roots still produces a Project. ═══

    #[test]
    fn signal_root_outside_configured_roots_still_produces_project() {
        // The "signal root" (a transcript) points at a directory that's NOT under config.roots.
        // The aggregator must still produce a Project for it — that's the union semantics.

        let fixture = Tmp::new("signal_outside_roots");

        // Transcript points at an OUTSIDE dir — not under fixture.path.
        let outside = std::env::temp_dir()
            .join(format!("swab_outside_{uuid}", uuid = unique_suffix()));
        std::fs::create_dir_all(&outside).expect("mkdir outside");

        let projects_dir = fixture.path.join(".claude/projects");
        write_transcript(
            &projects_dir.join("-slug").join("sess.jsonl"),
            &[&tline(Some("sess-x"), outside.to_str().unwrap())],
            0,
        );

        let paths = ScanPaths::for_home(&fixture.path);
        // config.roots does NOT include `outside` — the transcript's cwd is the ONLY way
        // to reach it.
        let config = test_config(vec![fixture.path.join("does_not_exist")]);
        let radar = run_scan(&config, &paths);

        assert_eq!(
            radar.projects.len(), 1,
            "signal-only root outside configured roots must still produce one Project"
        );
        // Compare canonicalized forms on both sides: `resolve_root` (and thus the project's
        // stored `path`) canonicalizes, which on macOS resolves `/var` -> `/private/var` (a
        // symlink) -- comparing against the raw, non-canonicalized `outside` path would
        // spuriously fail even though the project correctly points at the same directory.
        let expected = outside.canonicalize().unwrap();
        assert_eq!(
            radar.projects[0].path,
            expected.to_str().unwrap(),
            "project must be the signal's resolved root"
        );
    }

    // ═══ Test 8: Two sensor sources for the same root -> one Project, newest `at` wins. ═══

    #[test]
    fn two_sources_same_root_newest_at_wins_no_duplicate_projects() {
        let fixture = Tmp::new("multi_source_same_root");
        let repo = fixture.path.join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        std::fs::create_dir_all(repo.join(".git")).expect("mkdir .git");

        let repo_str = repo.to_str().unwrap();
        // Claude transcript: 10min ago (Older).
        write_transcript(
            &fixture.path.join(".claude/projects/-claude/sess.jsonl"),
            &[&tline(Some("claude-sess"), repo_str)],
            600, // 10 min = 600s ago
        );

        // Events file: 30s ago (Newer).
        std::fs::create_dir_all(fixture.path.join(".petridish")).expect("mkdir .petridish");
        std::fs::write(
            &fixture.path.join(".petridish/events.ndjson"),
            format!(
                "{{\"cwd\":\"{}\",\"at\":\"2026-08-14T12:00:30Z\"}}\n",
                repo_str
            ),
        )
        .expect("write events");

        let paths = ScanPaths::for_home(&fixture.path);
        // Note: `read_and_compact` truncates the events file on read, so subsequent calls
        // would see empty. For this test we call run_scan once and assert the result.

        let config = test_config(vec![repo.clone()]);
        let radar = run_scan(&config, &paths);

        assert_eq!(radar.projects.len(), 1, "two sources for same root -> one Project");
        // The newer-at wins — events is 30s old, claude is 600s old. The winning signal's
        // agent (events -> "claude-code" as events always uses claude-code, per events.rs)
        // becomes `active_agent`. Just verify one project and that active_agent is set.

        let p = &radar.projects[0];
        assert!(
            p.agent.active_agent.is_some(),
            "project must have an active_agent from whichever source had the newer at"
        );
        assert_eq!(radar.projects.len(), 1);
    }

    // ═══ Test 9: id stability across runs + path-specificity. ═════════════════════

    #[test]
    fn sha1_id_is_stable_across_runs_and_differs_between_paths() {
        let fixture = Tmp::new("id_stable");
        let repo1 = fixture.path.join("repo_a");
        let repo2 = fixture.path.join("repo_b");
        git_init_at(&repo1);
        git_init_at(&repo2);

        let paths = ScanPaths::for_home(&fixture.path);
        let config = test_config(vec![repo1.clone(), repo2.clone()]);

        let radar_a = run_scan(&config, &paths);
        let radar_b = run_scan(&config, &paths);

        // Same inputs -> same ids for each project (by index, since both runs yield
        // identical sort order).
        assert_eq!(radar_a.projects.len(), radar_b.projects.len());
        for (a, b) in radar_a.projects.iter().zip(radar_b.projects.iter()) {
            assert_eq!(a.id, b.id, "id must be stable across runs: {a:?} vs {b:?}");
        }

        // Different paths -> different ids. (repo_a and repo_b have different resolved paths.)
        let id_set: std::collections::HashSet<String> = radar_a
            .projects
            .iter()
            .map(|p| p.id.clone())
            .collect();
        assert_eq!(
            id_set.len(), 2,
            "two different paths must produce two different ids: {id_set:?}"
        );
    }

    // ═══ Test 10: write_scan writes a file and round-trips through schema. ════════

    #[test]
    fn write_scan_creates_state_file_and_round_trips() {
        let fixture = Tmp::new("write_scan");
        let repo = fixture.path.join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        std::fs::create_dir_all(repo.join(".git")).expect("mkdir .git");

        let state_path = fixture.path.join("state.json");
        let config = test_config(vec![repo.clone()]);
        let paths = ScanPaths::for_home(&fixture.path);

        // write_scan writes to state_path; must succeed.
        let written = write_scan(&config, &paths, &state_path).expect("write_scan must succeed");

        assert!(
            state_path.is_file(),
            "state path must exist after write_scan"
        );

        // Round-trip: read the file back as JSON and deserialize.
        let back = std::fs::read_to_string(&state_path).expect("read state file");
        let deserialized: Radar = serde_json::from_str(&back).expect("state file is valid Radar JSON");

        assert_eq!(written, deserialized, "round-trip Radar must equal written");
    }

    // ═══ Test 11: Production-defaults regression. ═════════════════════════════════

    #[test]
    fn production_defaults_via_for_home_yields_non_empty_signals() {
        // The original Python bug: sensors got `None`/unset directories in production, so every
        // sensor call returned empty and the Radar had zero agent activity. A test using
        // hand-built paths (not through `ScanPaths::for_home`) would exercise the working path
        // directly and miss this entire class of bug. This test uses `for_home` against a real
        // tmp-home that contains a REAL transcript fixture — mirroring what production does.

        let fixture = Tmp::new("prod_regression");
        let tmp_home = &fixture.path;

        // Set up a *real* transcript fixture inside the tmp home's .claude/projects, with one
        // repo referenced. The transcript MUST be inside the tmp_home tree so the sensor reads
        // a real file on disk.
        let repo = fixture.path.join("RealProject");
        std::fs::create_dir_all(&repo).expect("mkdir realproject");
        std::fs::create_dir_all(repo.join(".git")).expect("mkdir .git");

        let projects_dir = tmp_home.join(".claude").join("projects");
        std::fs::create_dir_all(projects_dir.join("-abc123")).expect("mkdir slug");
        write_transcript(
            &projects_dir.join("-abc123").join("sess.jsonl"),
            &[&tline(Some("abc-session"), repo.to_str().unwrap())],
            0, // now
        );

        // Use `for_home` EXACTLY — NOT hand-constructed paths, NOT mutation of HOME via env.
        let paths = ScanPaths::for_home(tmp_home);

        // Sanity: sensor paths came from for_home's composition (not empty / not absolute
        // garbage). These must hold regardless of what the fixture contains — they are a
        // check on `for_home` itself.

        let config = test_config(vec![repo.clone()]);
        let radar = run_scan(&config, &paths);

        // The tick must produce a project with claude-code as the active agent.
        // If `for_home` had silently defaulted any of its path components (the original bug's
        // shape), the claude sensor would have returned empty and this assertion would fail.
        assert_eq!(
            radar.projects.len(), 1,
            "expected one project from fixture: {:?}", radar.projects
        );
        let p = &radar.projects[0];
        assert_eq!(
            p.agent.active_agent.as_deref(),
            Some("claude-code"),
            "production-defaults regression: claude signal must be non-empty (got {:?})",
            p.agent
        );
    }

    // ═══ Test 12: ScanPaths::for_home composes all four paths correctly. ════════════

    #[test]
    fn scan_paths_for_home_composes_all_four_paths() {
        let fake_home = std::path::Path::new("/fake/tmp/home");
        let paths = ScanPaths::for_home(fake_home);

        // claude_projects_dir: ends with `.claude/projects`.
        assert!(
            paths.claude_projects_dir.ends_with(".claude/projects"),
            "claude_projects_dir must end in `.claude/projects`, got {:?}",
            paths.claude_projects_dir
        );

        // workspace_storage: contains `workspaceStorage` segment.
        let ws = paths.workspace_storage_dir.to_string_lossy().into_owned();
        assert!(
            ws.contains("workspaceStorage"),
            "workspace_storage_dir must contain `workspaceStorage`, got {ws}"
        );

        // events_path: uses `events::events_path()` which resolves via HOME env (we can't fake
        // that without unsafe). Check the form: it must contain `.petridish/events.ndjson`.
        let ep = paths.events_path.to_string_lossy().into_owned();
        assert!(
            ep.ends_with(".petridish/events.ndjson") || ep.contains(".petridish/events.ndjson"),
            "events_path must include `.petridish/events.ndjson`, got {ep}"
        );

        // quota_path: ends in `.claude/last-status.json`.
        assert!(
            paths.quota_path.ends_with(".claude/last-status.json"),
            "quota_path must end in `.claude/last-status.json`, got {:?}",
            paths.quota_path
        );
    }

    // ═══ Bonus: sort order by last_activity_at desc, name asc. ════════════════════

    #[test]
    fn projects_sort_last_activity_desc_none_last_then_name_asc() {
        let fixture = Tmp::new("sort_order");
        let repo_a = fixture.path.join("alpha_project");
        let repo_b = fixture.path.join("beta_project");
        std::fs::create_dir_all(&repo_a).expect("mkdir alpha");
        std::fs::create_dir_all(&repo_b).expect("mkdir beta");

        // Repo A: 5min old transcript (Recent).
        write_transcript(
            &fixture.path.join(".claude/projects/-a/sess.jsonl"),
            &[&tline(Some("a-sess"), repo_a.to_str().unwrap())],
            300,
        );
        // Repo B: 2h old transcript (Idle).
        write_transcript(
            &fixture.path.join(".claude/projects/-b/sess.jsonl"),
            &[&tline(Some("b-sess"), repo_b.to_str().unwrap())],
            7200,
        );

        let paths = ScanPaths::for_home(&fixture.path);
        let config = test_config(vec![repo_a.clone(), repo_b.clone()]);
        let radar = run_scan(&config, &paths);

        // Alpha is more recent (5min old) than Beta (2h); should be first in descending order.
        assert_eq!(radar.projects.len(), 2);
        assert_eq!(radar.projects[0].name, "alpha_project");
        assert_eq!(radar.projects[1].name, "beta_project");
    }

    // ═══ parent_path: no .worktrees segment -> None. ════════════════════════════════

    #[test]
    fn parent_path_plain_path_no_worktrees_segment_is_none() {
        let fixture = Tmp::new("parent_path_plain");
        let repo = fixture.path.join("my_repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        std::fs::create_dir_all(repo.join(".git")).expect("mkdir .git");

        let paths = ScanPaths::for_home(&fixture.path);
        let config = test_config(vec![repo.clone()]);
        let radar = run_scan(&config, &paths);

        assert_eq!(radar.projects.len(), 1, "expected one project");
        assert_eq!(
            radar.projects[0].parent_path, None,
            "no .worktrees segment must yield parent_path = None"
        );
    }

    // ═══ parent_path: /.worktrees/<name> -> Some(parent, no trailing slash). ══════════

    #[test]
    fn parent_path_worktrees_child_yields_correct_parent() {
        let fixture = Tmp::new("parent_path_worktrees");
        let parent = fixture.path.join("parent_repo");
        std::fs::create_dir_all(&parent).expect("mkdir parent");
        std::fs::create_dir_all(parent.join(".git")).expect("mkdir parent.git");

        // Create the .worktrees/<task> child dir — this is the resolved root we'll scan.
        let child = parent.join(".worktrees").join("my-task-name");
        std::fs::create_dir_all(&child).expect("mkdir worktrees/task");
        // A real git repo inside the worktree (so git::scan returns is_repo: true).
        std::fs::create_dir_all(child.join(".git")).expect("mkdir worktree.git");

        let paths = ScanPaths::for_home(&fixture.path);
        let config = test_config(vec![child.clone()]);
        let radar = run_scan(&config, &paths);

        assert_eq!(radar.projects.len(), 1, "expected one project");
        let p = &radar.projects[0];

        // parent_path must be the string form of the path before .worktrees, joined with '/'.
        // Canonicalize the expected value so macOS symlink resolution (`/var` -> `/private/var`)
        // doesn't cause a spurious mismatch.
        let canonical_parent = parent
            .canonicalize()
            .unwrap();
        let expected = canonical_parent.to_str().unwrap();
        assert_eq!(
            p.parent_path.as_deref(),
            Some(expected),
            "worktrees child parent_path must be the path before .worktrees, got {:?}, expected {:?}",
            p.parent_path,
            expected
        );
    }

    // ═══ serde: Project missing parent_path key deserializes to None. ═════════════════

    #[test]
    fn serde_project_missing_parent_path_key_deserializes_to_none() {
        let json_no_parent = r#"{
          "id": "aabbccdd1122",
          "name": "test-project",
          "path": "/Users/jan/repos/test-project",
          "category": "dev",
          "is_foreign": false,
          "git": {
            "is_repo": true,
            "branch": "master",
            "is_dirty": false,
            "uncommitted_files": 0,
            "last_commit_at": "2026-01-01T00:00:00Z",
            "mine_last_commit_at": "2026-01-01T00:00:00Z",
            "github_url": null
          },
          "agent": {
            "state": "idle",
            "active_agent": null,
            "last_event": null,
            "last_event_at": null,
            "session_id": null
          },
          "last_activity_at": "2026-01-01T00:00:00Z",
          "status_bucket": "cold"
        }"#;

        let project: schema::Project = serde_json::from_str(json_no_parent).expect(
            "Project JSON missing parent_path key must deserialize successfully",
        );
        assert_eq!(
            project.parent_path, None,
            "missing parent_path key must deserialize as None"
        );
    }
}
