//! Per-function ground-truth probe — the Rust half of parity_check.sh's oracle.
//!
//! Mirrors `swab/scripts/py_probe.py` subcommand-for-subcommand: reads one JSON object
//! of arguments from stdin, prints one JSON result to stdout. Exists so modules R2-R7 have
//! an external (Python-backed) correctness gate before the full aggregator (R8) makes
//! `diff_check.sh`'s whole-scan comparison possible. Do not add subcommands here without
//! adding the matching one in `py_probe.py` — parity_check.sh assumes the two are 1:1.

use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::PathBuf;
use swab::config::Config;
use swab::{discovery, events, git, schema, sensors};

fn read_stdin_json() -> Value {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .expect("failed to read stdin");
    serde_json::from_str(&buf).expect("stdin was not valid JSON")
}

fn str_vec(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn config_from_args(args: &Value) -> Config {
    let mut cfg = Config {
        roots: str_vec(args, "roots")
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        extra_paths: str_vec(args, "extra_paths")
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        author_patterns: str_vec(args, "author_patterns"),
        ..Config::default()
    };
    if let Some(since) = args.get("author_since").and_then(Value::as_str) {
        cfg.author_since = since.to_string();
    }
    let ignore = str_vec(args, "ignore_dirs");
    if !ignore.is_empty() {
        cfg.ignore_dirs = ignore.into_iter().collect::<HashSet<_>>();
    }
    if let Some(depth) = args.get("max_depth").and_then(Value::as_u64) {
        cfg.max_depth = depth as u32;
    }
    cfg
}

/// Truncates `at` to whole-second precision before serializing, matching the real
/// `projects.json` wire contract (schema.rs's write path). Without this, mtime-derived
/// fields spuriously mismatch py_probe.py's output due to float-vs-nanosecond precision
/// differences in how each language's stdlib reads back a file's mtime, even when both
/// sides observed the identical file.
fn signal_map_to_json(signals: HashMap<String, schema::AgentSignal>) -> Value {
    let map: serde_json::Map<String, Value> = signals
        .into_iter()
        .map(|(root, sig)| {
            let truncated_at = sig.at.format("%Y-%m-%dT%H:%M:%SZ").to_string();
            let mut value = serde_json::to_value(&sig).unwrap();
            if let Some(obj) = value.as_object_mut() {
                obj.insert("at".to_string(), Value::String(truncated_at));
            }
            (root, value)
        })
        .collect();
    Value::Object(map)
}

fn main() {
    let function = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: probe <resolve_root|is_foreign|git_scan|claude_scan|copilot_scan|events_read_and_compact|quota_read> < args.json");
        std::process::exit(2);
    });
    let args = read_stdin_json();

    let result = match function.as_str() {
        "resolve_root" => {
            let cwd = PathBuf::from(args["cwd"].as_str().expect("cwd required"));
            let cfg = config_from_args(&args);
            json!({ "root": discovery::resolve_root(&cwd, &cfg).display().to_string() })
        }
        "is_foreign" => {
            let path = PathBuf::from(args["path"].as_str().expect("path required"));
            let cfg = config_from_args(&args);
            json!({ "foreign": discovery::is_foreign(&path, &cfg) })
        }
        "git_scan" => {
            let path = PathBuf::from(args["path"].as_str().expect("path required"));
            let patterns = str_vec(&args, "author_patterns");
            let since = args
                .get("author_since")
                .and_then(Value::as_str)
                .unwrap_or("3 years");
            let gs = git::scan(&path, &patterns, since);
            serde_json::to_value(gs).unwrap()
        }
        "claude_scan" => {
            let dir = PathBuf::from(args["claude_dir"].as_str().expect("claude_dir required"));
            let cfg = config_from_args(&args);
            let cutoff = args
                .get("cold_cutoff_hours")
                .and_then(Value::as_u64)
                .unwrap_or(1440);
            signal_map_to_json(sensors::claude::scan(&dir, &cfg, cutoff))
        }
        "copilot_scan" => {
            let dir = PathBuf::from(
                args["workspace_storage_dir"]
                    .as_str()
                    .expect("workspace_storage_dir required"),
            );
            let cfg = config_from_args(&args);
            let cutoff = args
                .get("cold_cutoff_hours")
                .and_then(Value::as_u64)
                .unwrap_or(1440);
            signal_map_to_json(sensors::copilot::scan(&dir, &cfg, cutoff))
        }
        "events_read_and_compact" => {
            let path = PathBuf::from(args["path"].as_str().expect("path required"));
            let cfg = config_from_args(&args);
            let max_bytes = args
                .get("max_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(5_242_880);
            signal_map_to_json(events::read_and_compact(&path, &cfg, max_bytes).0)
        }
        "quota_read" => {
            let home = args.get("home").and_then(Value::as_str).map(PathBuf::from);
            let path = home
                .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap()))
                .join(".claude")
                .join("last-status.json");
            match sensors::quota::read_quota(&path) {
                Some(qs) => json!({ "quota": serde_json::to_value(qs).unwrap() }),
                None => json!({ "quota": null }),
            }
        }
        other => {
            eprintln!("unknown probe function: {other}");
            std::process::exit(2);
        }
    };

    println!("{result}");
}
