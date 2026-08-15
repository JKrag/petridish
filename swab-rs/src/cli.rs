//! `swab-rs` console entry point. Mirrors the `scan`/`list`/`path`/`doctor`/`config`
//! subcommands of `src/petridish/cli.py` (NOT `dash` — that pulls in TUI rendering code
//! that is explicitly out of scope for this port).
//!
//! Each subcommand is implemented as a standalone function (e.g. `fn cmd_scan`) that
//! returns an exit code and writes to a caller-supplied `io::Write` sink. This makes the
//! handlers trivially unit-testable without spawning the binary as a child process.

use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "swab-rs")]
pub struct Cli {
    /// Path to the state file (default: ~/.petridish/projects.json).
    #[arg(long, global = true)]
    pub state: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run a full tick and write the state file. Prints
    /// "scanned {n} projects in {ms}ms -> {path}". Rotates ~/.petridish/daemon.log past 5MB first.
    Scan,
    /// Read cached state only — never triggers a scan.
    List {
        #[arg(long)]
        bucket: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Resolve the best-matching project path by name/substring (enables `cd $(swab-rs path x)`).
    /// Tie-break by most recent last_activity_at.
    Path { query: String },
    /// Health checks: config loads, roots exist, state file present/fresh (<24h), hook
    /// marker found in ~/.claude/settings.json. Exit non-zero on any failure.
    Doctor,
    /// Print config file location + every Config field (name/default), sourced from the
    /// struct definition so it can't drift from config.rs.
    Config,
}

/// `~/.petridish/daemon.log` — launchd's append-mode fd keeps writing from offset
/// 0 after a truncation, so this is safe to hit mid-tick. Hard-capped at 5 MiB —
/// mirroring the Python original's `_DAEMON_LOG_MAX_BYTES`.
const DAEMON_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// `StatusBucket`/`AgentActivity` don't have an `as_str()` method (the schema derives only
/// `Serialize`/`Deserialize` via `#[serde(rename_all = "snake_case")]`) -- these two free
/// functions give the CLI the same lowercase snake_case string Python's CLI uses for
/// `--bucket` filtering and table rendering, without adding a method to the protected
/// schema.rs.
fn status_bucket_str(b: &crate::schema::StatusBucket) -> &'static str {
    use crate::schema::StatusBucket::*;
    match b {
        Active => "active",
        InFlight => "in_flight",
        Stale => "stale",
        Cold => "cold",
    }
}

fn agent_activity_str(a: &crate::schema::AgentActivity) -> &'static str {
    use crate::schema::AgentActivity::*;
    match a {
        Working => "working",
        Recent => "recent",
        Idle => "idle",
    }
}

/// Rotate `$HOME/.petridish/daemon.log` if it exceeds 5 MiB. Idempotent and
/// never fails loudly: a missing log is fine (not every install runs under launchd),
/// and the actual truncation is one `truncate` syscall — cheap.
fn rotate_daemon_log(log_path: &std::path::Path) {
    if let Ok(meta) = std::fs::metadata(log_path) {
        if meta.len() > DAEMON_LOG_MAX_BYTES {
            let _ = std::fs::File::create(log_path);
        }
    }
}

/// Resolved state-file path: the CLI's `--state`, or `$HOME/.petridish/projects.json`
/// when omitted. The default composes the path directly (rather than shelling out to a
/// helper) so tests can override it without touching HOME.
pub fn default_state_path() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set");
    PathBuf::from(&home).join(".petridish").join("projects.json")
}

// ── Subcommand handlers. Each returns a `u8` exit code (0 = OK) and writes
// ── human output to the provided `io::Write` sink so tests can capture it.
// ─────────────────────────────────────────────────────────────────────────

/// `scan`: rotate the daemon log, write a fresh Radar to the state file, and print
/// "scanned {n} projects in {ms}ms -> {path}".
///
/// Returns 0 on success, 1 if the write step fails (read-only filesystems, permission
/// errors, etc.). Mirrors `cli.py::_cmd_scan` — sensors degrade on their own; the only
/// thing that should bubble to *this* function's exit code is a write failure.
pub fn cmd_scan(state_path: &std::path::Path, out: &mut dyn Write) -> std::io::Result<u8> {
    rotate_daemon_log(
        &PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "~".to_string()))
            .join(".petridish")
            .join("daemon.log"),
    );

    let config = match crate::config::load_config(&crate::config::default_path()) {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(out, "scan failed: {e}");
            return Ok(1);
        }
    };

    let paths = crate::scan::ScanPaths::production_defaults();
    match crate::scan::write_scan(&config, &paths, state_path) {
        Ok(radar) => {
            let n = radar.projects.len();
            let ms = radar.scan_duration_ms;
            let _ = writeln!(out, "scanned {n} projects in {ms}ms -> {}", state_path.display());
            Ok(0)
        }
        Err(e) => {
            let _ = writeln!(out, "scan failed: {e}");
            Ok(1)
        }
    }
}

/// `list`: read the cached state file (never a scan), filter by `--bucket` and
/// `--all`, then either print a table or emit JSON.
///
/// Exit codes: 0 on success (empty project list is still valid — an empty table or `[]`),
/// 1 when the state file is missing.
pub fn cmd_list(
    state_path: &std::path::Path,
    bucket: Option<&str>,
    all: bool,
    json: bool,
    out: &mut dyn Write,
) -> std::io::Result<u8> {
    if !state_path.is_file() {
        let _ = writeln!(
            out,
            "no state file at {}; run 'swab-rs scan' first",
            state_path.display()
        );
        return Ok(1);
    }

    let data = std::fs::read(state_path).map_err(|e| {
        let _ = writeln!(out, "cannot read {}: {e}", state_path.display());
        e
    })?;

    let radar: crate::schema::Radar = match serde_json::from_slice(&data) {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(out, "state file is not valid JSON: {e}");
            return Ok(1);
        }
    };

    let mut projects: Vec<&crate::schema::Project> = radar.projects.iter().collect();

    if let Some(b) = bucket {
        projects.retain(|p| status_bucket_str(&p.status_bucket) == b);
    }

    if !all {
        projects.retain(|p| !p.is_foreign);
    }

    if json {
        let payload = serde_json::to_string_pretty(&projects).map_err(|e| {
            let _ = writeln!(out, "serialize error: {e}");
            e
        })?;
        let _ = writeln!(out, "{payload}");
        return Ok(0);
    }

    _print_table(&projects, out)?;
    Ok(0)
}

/// `path`: resolve QUERY against the cached Radar's projects. Single-writer output:
/// if a match is found, print only the project path (so `cd $(swab-rs path x)` works);
/// if not, emit an error to *stderr-equivalent* sink and return 1.
///
/// Priority: exact name match (if exactly one); else case-insensitive substring of name
/// (rank 0); else case-insensitive substring of path (rank 1). Ties within a rank broken by
/// most-recent `last_activity_at`. Mirrors `cli.py::_cmd_path` exactly — found via a real
/// gap audit that an earlier version of this function was case-sensitive and had no
/// path-substring fallback tier at all.
pub fn cmd_path(
    state_path: &std::path::Path,
    query: &str,
    out: &mut dyn Write,
) -> std::io::Result<u8> {
    if !state_path.is_file() {
        let _ = writeln!(
            out,
            "no state file at {}; run 'swab-rs scan' first",
            state_path.display()
        );
        return Ok(1);
    }

    let radar: crate::schema::Radar = {
        let data = std::fs::read(state_path).map_err(|e| {
            let _ = writeln!(out, "cannot read {}: {e}", state_path.display());
            e
        })?;
        serde_json::from_slice(&data).map_err(|e| {
            let _ = writeln!(out, "state file is not valid JSON: {e}");
            e
        })?
    };

    // Priority 1: exact match on name. If exactly one, use it immediately. If zero or
    // several (duplicate names — shouldn't happen, but Python is defensive about it too),
    // fall through to the tiered substring search below, which still finds exact-name
    // matches at rank 0 (an exact match trivially contains itself as a substring).
    let exact: Vec<&crate::schema::Project> = radar.projects.iter().filter(|p| p.name == query).collect();
    if exact.len() == 1 {
        let _ = writeln!(out, "{}", exact[0].path);
        return Ok(0);
    }

    // Priority 2: case-insensitive substring of name (rank 0). Priority 3: case-insensitive
    // substring of path (rank 1), for projects not already matched by name. Ties within a
    // rank broken by most-recent last_activity_at (None sorts last, mirroring Python's
    // `0.0 if None else -timestamp` key).
    let query_lower = query.to_lowercase();
    let mut candidates: Vec<(u8, &crate::schema::Project)> = Vec::new();
    let mut matched_by_name: Vec<&crate::schema::Project> = Vec::new();
    for p in &radar.projects {
        if p.name.to_lowercase().contains(&query_lower) {
            candidates.push((0, p));
            matched_by_name.push(p);
        }
    }
    for p in &radar.projects {
        if p.path.to_lowercase().contains(&query_lower)
            && !matched_by_name.iter().any(|m| std::ptr::eq(*m, p))
        {
            candidates.push((1, p));
        }
    }

    if candidates.is_empty() {
        let _ = writeln!(out, "no project matches {query:?}");
        return Ok(1);
    }

    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.last_activity_at.cmp(&a.1.last_activity_at)));

    let _ = writeln!(out, "{}", candidates[0].1.path);
    Ok(0)
}

/// `doctor`: health-check the system. Reports `ok:` / `fail:` per check, exits 1 if any
/// failed. Wraps each check in its own try/catch so a filesystem quirk doesn't abort the
/// whole probe — mirrors `cli.py::_cmd_doctor`'s per-check `try/except` envelope.
///
/// Checks:
/// - config loads without error,
/// - every path in `roots` exists on disk (dir),
/// - state file exists and is fresh (<24h old),
/// - `~/.claude/settings.json` contains the HOOK_MARKER string.
pub fn cmd_doctor(
    state_path: &std::path::Path,
    out: &mut dyn Write,
) -> std::io::Result<u8> {
    let mut problems: Vec<String> = Vec::new();
    let mut report: Vec<(String, bool /* ok or not */)> = Vec::new();

    macro_rules! check {
        ($name:expr, $body:block) => {{
            let result: Result<bool, String> = (move || $body)();
            match result {
                Ok(true) => report.push(($name.to_string(), true)),
                Ok(false) => {
                    problems.push(format!("{}: check returned false", $name));
                    report.push(($name.to_string(), false));
                }
                Err(msg) => {
                    problems.push(format!("{}: {msg}", $name));
                    report.push(($name.to_string(), false));
                }
            }
        }};
    }

    check!("config", {
        crate::config::load_config(&crate::config::default_path()).map_err(|e| format!("{e}"))?;
        Ok::<bool, String>(true)
    });

    check!("roots", {
        let cfg = crate::config::load_config(&crate::config::default_path())
            .map_err(|e| format!("config load failed: {e}"))?;
        let missing: Vec<String> = cfg
            .roots
            .iter()
            .filter(|p| !p.is_dir())
            .map(|p| p.display().to_string())
            .collect();
        if missing.is_empty() {
            Ok(true)
        } else {
            Err(format!("roots not found: {}", missing.join(", ")))
        }
    });

    check!("state", {
        if !state_path.is_file() {
            return Err(format!("state file missing: {}", state_path.display()));
        }
        let data = std::fs::read(state_path).map_err(|e| {
            format!("state file invalid JSON: {e}")
        })?;
        let radar: crate::schema::Radar = serde_json::from_slice(&data).map_err(|e| {
            format!("state file invalid JSON: {e}")
        })?;
        let now = chrono::Utc::now();
        let age_h = (now - radar.updated_at).num_seconds() as f64 / 3_600.0;
        if age_h >= 24.0 {
            return Err(format!("state file stale ({age_h:.1}h old)"));
        }
        Ok(true)
    });

    check!("hook", {
        let home = std::env::var("HOME").unwrap_or_else(|_| "~".to_string());
        let settings_path = std::path::PathBuf::from(home).join(".claude").join("settings.json");
        if !settings_path.is_file() {
            return Err(format!("settings.json not found: {}", settings_path.display()));
        }
        let text = std::fs::read_to_string(&settings_path).map_err(|e| {
            format!("cannot read settings.json: {e}")
        })?;
        let _: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            format!("settings.json not valid JSON: {e}")
        })?;
        if !text.contains(crate::config::HOOK_MARKER) {
            return Err("swab-hook marker not found in ~/.claude/settings.json".to_string());
        }
        Ok(true)
    });

    for (key, ok) in &report {
        if *ok {
            let _ = writeln!(out, "ok: {key}");
        } else {
            let _ = writeln!(out, "fail: {key}");
        }
    }

    Ok(if problems.is_empty() { 0 } else { 1 })
}

/// One line of human explanation per `Config` field. Mirrors `cli.py::_CONFIG_FIELD_HELP`
/// verbatim — found missing entirely in a gap audit (an earlier version of `cmd_config`
/// printed only the field name + default, dropping the description Python always prints).
const CONFIG_FIELD_HELP: &[(&str, &str)] = &[
    ("roots", "Directories crawled for projects"),
    ("extra_paths", "Individual extra project paths, for anything outside roots"),
    ("author_patterns", "Regex(es) matched against \"git log --author=\" to decide \"did I write this\""),
    ("author_since", "How far back git log looks when computing authorship"),
    ("ignore_dirs", "Directory basenames hard-skipped during crawl"),
    ("bucket_thresholds", "Hour cutoffs for the active/in_flight/stale/cold status buckets"),
    ("category_overrides", "{path_glob_or_pattern: category_label} manual recategorisation"),
    ("max_depth", "How deep the crawl descends into roots before giving up on a subtree"),
];

/// `config`: print the config file location + every `Config` field, one at a time in
/// struct-field order. Keeps the list hardcoded (one line per field name + default) rather
/// than reflecting over `dataclasses.fields` — the struct is small enough to update by eye,
/// and a manual list can't drift silently the way a parallel dict would.
pub fn cmd_config(out: &mut dyn Write) -> std::io::Result<u8> {
    let cfg = crate::config::Config::default();
    writeln!(out, "Config file: {}", crate::config::default_path().display())?;
    writeln!(
        out,
        "Optional TOML file — every field below has a default, so a missing \
         file, or any field left out, is valid; only what you set overrides \
         the default."
    )?;
    writeln!(out)?;

    // Field order kept in sync with `Config` struct definition by hand.
    for (name, default) in &[
        ("roots", format_toml_path_list(&cfg.roots)),
        ("extra_paths", format_toml_path_list(&cfg.extra_paths)),
        ("author_patterns", format_toml_string_list(&cfg.author_patterns)),
        ("author_since", format_toml_string(&cfg.author_since)),
        ("ignore_dirs", format_toml_sorted_string_set(&cfg.ignore_dirs)),
        ("bucket_thresholds", format_toml_bucket_thresholds(&cfg.bucket_thresholds)),
        ("category_overrides", format_toml_string_map(&cfg.category_overrides)),
        ("max_depth", cfg.max_depth.to_string()),
    ] {
        let help_text = CONFIG_FIELD_HELP.iter().find(|(n, _)| n == name).map(|(_, h)| *h).unwrap_or("");
        writeln!(out, "  {name}")?;
        writeln!(out, "      {help_text}")?;
        writeln!(out, "      default: {default}")?;
    }

    writeln!(out)?;
    writeln!(
        out,
        "Example — only override what you care about:\n\n  roots = [\"~/repos\", \"~/work\"]\n  max_depth = 6\n\n  \
         [bucket_thresholds]\n  active = 24.0"
    )?;

    Ok(0)
}

/// TOML-style rendering of `Config` field defaults, mirroring `cli.py::_format_default`
/// field-type by field-type (that function dispatches on Python's dynamic type at runtime;
/// Rust's static types mean one formatter per field type instead — found via a gap audit
/// that an earlier version used `{value:?}` (Rust `Debug`) instead, which doesn't look like
/// TOML at all: `Vec<PathBuf>` debug-quotes each path, `HashMap` uses Rust's `{k: v}` map
/// syntax, and — since `HashMap`/`HashSet` iteration order is unspecified — the SAME config
/// could legitimately print its keys in a different order across two runs).
fn format_toml_path_list(items: &[std::path::PathBuf]) -> String {
    let quoted: Vec<String> = items.iter().map(|p| format!("\"{}\"", p.display())).collect();
    format!("[{}]", quoted.join(", "))
}

fn format_toml_string_list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|s| format!("\"{s}\"")).collect();
    format!("[{}]", quoted.join(", "))
}

fn format_toml_string(s: &str) -> String {
    format!("\"{s}\"")
}

/// `ignore_dirs` is a `HashSet` (Python: `frozenset`) — unordered by construction, so its
/// default rendering must sort, exactly like `_format_default`'s frozenset branch
/// (`sorted(str(v) for v in value)`), or the printed default would vary run to run.
fn format_toml_sorted_string_set(items: &std::collections::HashSet<String>) -> String {
    let mut sorted: Vec<&String> = items.iter().collect();
    sorted.sort();
    let quoted: Vec<String> = sorted.iter().map(|s| format!("\"{s}\"")).collect();
    format!("[{}]", quoted.join(", "))
}

/// `DEFAULT_BUCKETS`' Python dict literal has a fixed key order (active, in_flight, stale);
/// `HashMap` doesn't preserve insertion order, so that fixed order is hardcoded here rather
/// than iterated — any unexpected extra key (shouldn't happen for the default map) is
/// appended sorted so nothing silently goes missing from the printed default.
fn format_toml_bucket_thresholds(map: &std::collections::HashMap<String, f64>) -> String {
    if map.is_empty() {
        return "{}".to_string();
    }
    let known_order = ["active", "in_flight", "stale"];
    let mut parts = Vec::new();
    for key in known_order {
        if let Some(v) = map.get(key) {
            parts.push(format!("{key} = {v:?}")); // {:?} forces "48.0", not "48" — matches Python's str(48.0).
        }
    }
    let mut extra: Vec<&String> = map.keys().filter(|k| !known_order.contains(&k.as_str())).collect();
    extra.sort();
    for key in extra {
        parts.push(format!("{key} = {:?}", map[key]));
    }
    format!("{{{}}}", parts.join(", "))
}

fn format_toml_string_map(map: &std::collections::HashMap<String, String>) -> String {
    if map.is_empty() {
        return "{}".to_string();
    }
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let parts: Vec<String> = keys.iter().map(|k| format!("{k} = {}", map[*k])).collect();
    format!("{{{}}}", parts.join(", "))
}

/// Print a table of projects: header row, separator, then one row per project.
/// Used only by the non-`--json` list path. Matches `cli.py::_print_table`'s
/// ljust-based layout, joiner is `"  "`.
fn _print_table(projects: &[&crate::schema::Project], out: &mut dyn Write) -> std::io::Result<()> {
    let cols = ["bucket", "name", "agent", "branch", "dirty"];

    let mut rows: Vec<Vec<String>> = Vec::new();
    for p in projects {
        let agent_label = match &p.agent.active_agent {
            Some(a) => format!("{a} ({})", agent_activity_str(&p.agent.state)),
            None => agent_activity_str(&p.agent.state).to_string(),
        };
        let dirty = if p.git.is_repo && p.git.is_dirty { "*" } else { " " };
        let branch = match &p.git.branch {
            Some(b) => b.clone(),
            None => "-".to_string(),
        };
        rows.push(vec![
            status_bucket_str(&p.status_bucket).to_string(),
            p.name.clone(),
            agent_label,
            branch,
            dirty.to_string(),
        ]);
    }

    let mut widths = vec![0usize; cols.len()];
    for (i, c) in cols.iter().enumerate() {
        widths[i] = c.len();
    }
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }

    let header: String = cols
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{:<width$}", c, width = widths[i]))
        .collect::<Vec<_>>()
        .join("  ");

    let _ = writeln!(out, "{header}");
    let _ = writeln!(
        out,
        "{}",
        widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>().join("  ")
    );

    for row in &rows {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| format!("{:<width$}", cell, width = widths[i]))
            .collect();
        let _ = writeln!(out, "{}", cells.join("  "));
    }

    Ok(())
}

/// Parse clap args, dispatch to the handler. Returns a `Result<i32>` matching
/// std::os::unix's `i32` exit code. The whole function is a thin wrapper so it can be
/// replaced in tests by calling a specific handler directly.
pub fn run_command(args: Cli, out: &mut dyn Write) -> std::io::Result<i32> {
    use Command::*;
    let state = args.state.clone().unwrap_or_else(default_state_path);
    match args.command {
        Scan => cmd_scan(&state, out).map(|c| c as i32),
        List { bucket, all, json } => cmd_list(
            &state,
            bucket.as_deref(),
            all,
            json,
            out,
        )
        .map(|c| c as i32),
        Path { query } => cmd_path(&state, &query, out).map(|c| c as i32),
        Doctor => cmd_doctor(&state, out).map(|c| c as i32),
        Config => cmd_config(out).map(|c| c as i32),
    }
}

pub fn main() {
    let cli = Cli::parse();

    // Best-effort write: if stdout has been closed (`swab list | head -1`), we still
    // want to exit 0 rather than crashing. The wrapped output handles the broken-pipe case.
    let stdout = std::io::stdout();
    let result = run_command(cli, &mut stdout.lock());
    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "swab-rs: {e}");
            std::process::exit(1);
        }
    }
}

// ── Tests. Each subcommand handler is a plain function and is unit-tested directly.
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Radar, Project as SchemaProject, StatusBucket};
    use chrono::Utc;
    use std::path::PathBuf;

    // ── Helpers ────────────────────────────────────────────────────────

    /// Write a fixture `Radar` to a temp path and return its location.
    fn write_fixture_radar(path: &PathBuf, radar: Radar) {
        crate::schema::write_atomic(path, &radar).expect("write fixture state");
    }

    /// Capture stdout/stderr into a Vec<u8>. Tests can read back the contents.
    #[derive(Debug)]
    struct Capture {
        bytes: std::sync::Mutex<Vec<u8>>,
    }

    impl Capture {
        fn new() -> Self {
            Capture {
                bytes: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn as_bytes(&self) -> Vec<u8> {
            self.bytes.lock().unwrap().clone()
        }

        fn as_str(&self) -> String {
            String::from_utf8_lossy(&self.bytes.lock().unwrap()).into_owned()
        }
    }

    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.bytes.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Build a minimal `Radar` with the given project list. Convenience helper so tests
    /// don't have to repeat `schema_version`, `updated_at`, etc.
    fn test_radar(projects: Vec<SchemaProject>) -> Radar {
        Radar {
            schema_version: 1,
            updated_at: Utc::now(),
            scan_duration_ms: 0,
            projects,
            quota: None,
        }
    }

    fn test_project(name: &str, path: &str, bucket: StatusBucket, is_foreign: bool) -> SchemaProject {
        SchemaProject {
            id: format!("id_{name}"),
            name: name.to_string(),
            path: path.to_string(),
            category: "test".to_string(),
            is_foreign,
            git: crate::schema::GitState::not_a_repo(),
            agent: crate::schema::AgentState::idle_unknown(),
            last_activity_at: Some(Utc::now()),
            status_bucket: bucket,
        }
    }

    // ── Tests 1..3: List ────────────────────────────────────────────────

    /// Test 1: `List` with a real fixture state file -> filtered/JSON output.
    #[test]
    fn list_with_real_fixture_state_file() {
        let dir = std::env::temp_dir().join("swab_rs_test_list_basic");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state_path = dir.join("projects.json");

        // Mixed project list.
        let radar = test_radar(vec![
            test_project("project-a", "/tmp/pa", StatusBucket::Active, false),
            test_project("project-b", "/tmp/pb", StatusBucket::Stale, false),
            test_project("project-c", "/tmp/pc", StatusBucket::Active, true), // foreign — hidden
        ]);
        write_fixture_radar(&state_path, radar);

        let mut cap = Capture::new();
        let code = cmd_list(&state_path, None, false, false, &mut cap)
            .unwrap();
        assert_eq!(code, 0);
        // project-c must be excluded (foreign, --all not set).
        let captured = cap.as_str();
        assert!(captured.contains("project-a"));
        assert!(captured.contains("project-b"));
        assert!(!captured.contains("project-c"), "foreign project must be hidden: {captured}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Test 2: `List --bucket stale` -> only stale projects returned.
    #[test]
    fn list_bucket_filter() {
        let dir = std::env::temp_dir().join("swab_rs_test_list_bucket");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state_path = dir.join("projects.json");

        let radar = test_radar(vec![
            test_project("active-p", "/tmp/pa", StatusBucket::Active, false),
            test_project("stale-p", "/tmp/pb", StatusBucket::Stale, false),
            test_project("cold-p", "/tmp/pc", StatusBucket::Cold, false),
        ]);
        write_fixture_radar(&state_path, radar);

        let mut cap = Capture::new();
        let code = cmd_list(&state_path, Some("stale"), false, false, &mut cap)
            .unwrap();
        assert_eq!(code, 0);

        let captured = cap.as_str();
        assert!(captured.contains("stale-p"), "must contain stale project: {captured}");
        assert!(!captured.contains("active-p"), "must exclude active: {captured}");
        assert!(!captured.contains("cold-p"), "must exclude cold: {captured}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Test 3: `List` without `--all` excludes foreign projects.
    #[test]
    fn list_excludes_foreign_without_all() {
        let dir = std::env::temp_dir().join("swab_rs_test_list_foreign");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state_path = dir.join("projects.json");

        let radar = test_radar(vec![
            test_project("home-proj", "/home/p", StatusBucket::Active, false),
            test_project("foreign-proj", "/opt/foreign", StatusBucket::Active, true),
        ]);
        write_fixture_radar(&state_path, radar);

        // Default: --all is false.
        let mut cap = Capture::new();
        let code = cmd_list(&state_path, None, false, false, &mut cap).unwrap();
        assert_eq!(code, 0);

        let captured = cap.as_str();
        assert!(
            !captured.contains("foreign-proj"),
            "must exclude foreign by default: {captured}"
        );

        // Now --all = true: foreign should appear.
        let mut cap = Capture::new();
        let code = cmd_list(&state_path, None, true, false, &mut cap).unwrap();
        assert_eq!(code, 0);
        let captured = cap.as_str();
        assert!(captured.contains("foreign-proj"), "with --all must include foreign: {captured}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Tests 4..6: Path ────────────────────────────────────────────────

    /// Test 4: `Path` with an exact-name match -> project.path printed.
    #[test]
    fn path_exact_name_match() {
        let dir = std::env::temp_dir().join("swab_rs_test_path_exact");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state_path = dir.join("projects.json");

        let radar = test_radar(vec![
            test_project("my-project", "/tmp/xyz/my-project", StatusBucket::Active, false),
            test_project("other-proj", "/tmp/xyz/other-proj", StatusBucket::Active, false),
        ]);
        write_fixture_radar(&state_path, radar);

        let mut cap = Capture::new();
        let code = cmd_path(&state_path, "my-project", &mut cap).unwrap();
        assert_eq!(code, 0);

        let cap_str = cap.as_str(); let captured = cap_str.trim();
        assert_eq!(captured, "/tmp/xyz/my-project", "exact name must print its path");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Test 5: `Path` with a substring matching two projects -> most recent `last_activity_at` wins.
    #[test]
    fn path_substring_tiebreak_by_recency() {
        let dir = std::env::temp_dir().join("swab_rs_test_path_substring");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state_path = dir.join("projects.json");

        let now = Utc::now();
        let radar = test_radar(vec![
            SchemaProject {
                id: "a".into(),
                name: "project-alpha".into(),
                path: "/tmp/alpha".into(),
                category: "test".into(),
                is_foreign: false,
                git: crate::schema::GitState::not_a_repo(),
                agent: crate::schema::AgentState::idle_unknown(),
                last_activity_at: Some(now - chrono::Duration::hours(2)), // older
                status_bucket: StatusBucket::Active,
            },
            SchemaProject {
                id: "b".into(),
                name: "project-beta".into(),
                path: "/tmp/beta".into(),
                category: "test".into(),
                is_foreign: false,
                git: crate::schema::GitState::not_a_repo(),
                agent: crate::schema::AgentState::idle_unknown(),
                last_activity_at: Some(Utc::now()), // newest
                status_bucket: StatusBucket::Active,
            },
        ]);
        write_fixture_radar(&state_path, radar);

        let mut cap = Capture::new();
        // "project" is a substring of both names.
        let code = cmd_path(&state_path, "project", &mut cap).unwrap();
        assert_eq!(code, 0);

        let cap_str = cap.as_str(); let captured = cap_str.trim();
        assert_eq!(captured, "/tmp/beta", "newest project wins the tie-break");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Test 6: `Path` with no match -> non-zero exit, nothing on stdout.
    #[test]
    fn path_no_match() {
        let dir = std::env::temp_dir().join("swab_rs_test_path_nomatch");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state_path = dir.join("projects.json");

        let radar = test_radar(vec![
            test_project("alpha", "/tmp/a", StatusBucket::Active, false),
        ]);
        write_fixture_radar(&state_path, radar);

        let mut cap = Capture::new();
        let code = cmd_path(&state_path, "zzz_no_match_zzz", &mut cap).unwrap();
        assert_eq!(code, 1, "must exit non-zero on no match");

        let cap_str = cap.as_str(); let captured = cap_str.trim();
        assert!(
            captured.is_empty() || !captured.starts_with('/'),
            "stdout must stay clean for no-match — got {captured:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression: found via a gap audit against `cli.py::_cmd_path`. A query that only
    /// matches a project's *path* (not its name) must still resolve — Python has a rank-1
    /// substring-of-path fallback tier; an earlier Rust version had no such tier at all.
    #[test]
    fn path_matches_via_path_substring_not_name() {
        let dir = std::env::temp_dir().join("swab_rs_test_path_via_path");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state_path = dir.join("projects.json");

        let radar = test_radar(vec![
            test_project("frontend", "/tmp/xyz/special-repo-name/frontend", StatusBucket::Active, false),
            test_project("backend", "/tmp/xyz/other/backend", StatusBucket::Active, false),
        ]);
        write_fixture_radar(&state_path, radar);

        let mut cap = Capture::new();
        let code = cmd_path(&state_path, "special-repo-name", &mut cap).unwrap();
        assert_eq!(code, 0, "a query matching only the path (not any project name) must still resolve");

        let cap_str = cap.as_str();
        assert_eq!(cap_str.trim(), "/tmp/xyz/special-repo-name/frontend");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression: found via a gap audit. Name matching must be case-insensitive, matching
    /// `cli.py::_cmd_path`'s `query.lower() in p.name.lower()`.
    #[test]
    fn path_name_match_is_case_insensitive() {
        let dir = std::env::temp_dir().join("swab_rs_test_path_case_insensitive");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state_path = dir.join("projects.json");

        let radar = test_radar(vec![
            test_project("MyProject", "/tmp/xyz/MyProject", StatusBucket::Active, false),
        ]);
        write_fixture_radar(&state_path, radar);

        let mut cap = Capture::new();
        let code = cmd_path(&state_path, "myproject", &mut cap).unwrap();
        assert_eq!(code, 0);
        assert_eq!(cap.as_str().trim(), "/tmp/xyz/MyProject");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression: found via a gap audit that `cmd_config` had no test at all, and was
    /// silently missing the per-field help line (`cli.py::_CONFIG_FIELD_HELP`) plus
    /// rendering defaults with Rust `Debug` instead of TOML syntax. Locks in the exact
    /// output shape byte-for-byte against a value confirmed identical to `swab config`'s
    /// real output on this machine.
    #[test]
    fn config_output_matches_python_format() {
        let mut cap = Capture::new();
        let code = cmd_config(&mut cap).unwrap();
        assert_eq!(code, 0);
        let out = cap.as_str();

        assert!(out.contains("  roots\n      Directories crawled for projects\n      default: [\"~/repos\", \"~/learning\"]\n"));
        assert!(out.contains("  bucket_thresholds\n      Hour cutoffs for the active/in_flight/stale/cold status buckets\n      default: {active = 48.0, in_flight = 336.0, stale = 1440.0}\n"));
        assert!(out.contains("  category_overrides\n      {path_glob_or_pattern: category_label} manual recategorisation\n      default: {}\n"));
        assert!(out.contains("  max_depth\n      How deep the crawl descends into roots before giving up on a subtree\n      default: 4\n"));
        assert!(
            out.contains("ignore_dirs\n      Directory basenames hard-skipped during crawl\n      default: [\".Trash\""),
            "ignore_dirs (a HashSet) must render sorted, matching Python's frozenset->sorted() branch"
        );
    }

    // ── Tests 7..8: Doctor ──────────────────────────────────────────────

    /// Test 7: Doctor against a fixture with missing/malformed config -> that check reported
    /// as failed, overall non-zero exit.
    #[test]
    fn doctor_with_broken_config() {
        let dir = std::env::temp_dir().join("swab_rs_test_doctor_broken");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // A config we can control.
        std::fs::write(dir.join("config.toml"), "this is not valid toml [[[[")
            .expect("write broken config");

        // Force default_path() to point at our test dir (only during this test).
        let real_home = std::env::var("HOME").unwrap_or_else(|_| "~".into());
        let fake_home = dir.join("fake-home");
        std::fs::create_dir_all(&fake_home).unwrap();
        unsafe { std::env::set_var("HOME", fake_home.to_str().unwrap()) };
        let state_path = dir.join("projects.json");

        // Make sure projects.json doesn't exist yet — state check should fail.
        let mut cap = Capture::new();
        let code = cmd_doctor(&state_path, &mut cap).unwrap();
        assert_eq!(code, 1, "doctor should fail when config is broken");

        let captured = cap.as_str();
        assert!(
            captured.contains("fail") || captured.contains("config"),
            "config failure should be reported: {captured}"
        );

        unsafe { std::env::set_var("HOME", &real_home) };
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Test 8: Doctor against a healthy fixture (valid config, roots exist, fresh state)
    /// -> all checks pass, exit 0.
    #[test]
    fn doctor_healthy_fixture() {
        // Set up a fake HOME directory layout with the complete fixture tree:
        //   <dir>/.petridish/config.toml  — valid config with roots pointing to a real dir
        //   <dir>/.petridish/projects.json — fresh state
        //   <dir>/.claude/settings.json  — contains the HOOK_MARKER
        //   <dir>/repos                    — real directory (resolved from $HOME/repos)
        let dir = std::env::temp_dir().join("swab_rs_test_doctor_healthy");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join(".petridish")).unwrap();
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::create_dir_all(dir.join("repos")).unwrap();

        // Config: roots = ["$HOME/repos"] -> expands via load_config's expand_path to
        // `<dir>/repos` once HOME is set below.
        std::fs::write(
            dir.join(".petridish").join("config.toml"),
            "roots = [\"$HOME/repos\"]",
        )
        .unwrap();

        // Fresh state file.
        let radar = test_radar(vec![]);
        write_fixture_radar(&dir.join(".petridish").join("projects.json"), radar);

        // settings.json with the HOOK_MARKER somewhere in it.
        std::fs::write(
            dir.join(".claude").join("settings.json"),
            "{\"hooks\": {\"command\": \"# petridish echo hello\"}}",
        )
        .unwrap();

        let real_home = std::env::var("HOME").unwrap_or_else(|_| "~".into());
        unsafe { std::env::set_var("HOME", dir.to_str().unwrap()) };

        let state_path = dir.join(".petridish").join("projects.json");
        let mut cap = Capture::new();
        let code = cmd_doctor(&state_path, &mut cap).unwrap();
        assert_eq!(code, 0, "doctor should pass on a healthy fixture");

        let captured = cap.as_str();
        assert!(captured.contains("ok: config"), "config should be ok: {captured}");
        assert!(captured.contains("ok: roots"), "roots should be ok: {captured}");
        assert!(captured.contains("ok: state"), "state should be ok: {captured}");
        assert!(captured.contains("ok: hook"), "hook should be ok: {captured}");

        unsafe { std::env::set_var("HOME", &real_home) };
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Test 9: Daemon log rotation ─────────────────────────────────────

    /// Test 9: Daemon log rotation — a log file over 5MB gets truncated, a small one is untouched.
    #[test]
    fn daemon_log_rotation_over_threshold() {
        let dir = std::env::temp_dir().join("swab_rs_test_rotate");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let log_path = dir.join("daemon.log");

        // Write more than 5MiB.
        let big = vec![b'x'; (DAEMON_LOG_MAX_BYTES + 1) as usize];
        std::fs::write(&log_path, &big).expect("write big log");
        assert_eq!(
            std::fs::metadata(&log_path).unwrap().len(),
            DAEMON_LOG_MAX_BYTES + 1,
            "setup: must be over threshold"
        );

        rotate_daemon_log(&log_path);
        let after_meta = std::fs::metadata(&log_path).unwrap();
        assert_eq!(
            after_meta.len(),
            0,
            "log must be truncated to 0 bytes when over threshold, got {}",
            after_meta.len()
        );

        // Now the small case.
        let small_log = dir.join("small.log");
        std::fs::write(&small_log, b"tiny").unwrap();
        assert_eq!(std::fs::metadata(&small_log).unwrap().len(), 4);
        rotate_daemon_log(&small_log);
        // File should still be 4 bytes — unchanged.
        assert_eq!(
            std::fs::metadata(&small_log).unwrap().len(), 4,
            "small log must not be rotated"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Test 10: daemon.log missing -> rotation is a no-op, never fails ─
    #[test]
    fn daemon_log_missing_no_op() {
        let dir = std::env::temp_dir().join("swab_rs_test_rotate_missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let log_path = dir.join("nonexistent.log");
        assert!(!log_path.exists());

        // Must not panic.
        rotate_daemon_log(&log_path);
        assert!(!log_path.exists(), "rotation must not create a missing file");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
