//! `~/.petridish/config.toml` loader. Mirrors `src/petridish/config.py`.
//! Entirely optional file — every field has a default; a missing file is valid,
//! a malformed one degrades field-by-field (never aborts the whole load).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Literal marker string appended to hook `command` entries in `~/.claude/settings.json`;
/// shared with `swab-rs doctor`'s hook-detection.
pub const HOOK_MARKER: &str = "# petridish";

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub roots: Vec<PathBuf>,
    pub extra_paths: Vec<PathBuf>,
    pub author_patterns: Vec<String>,
    pub author_since: String,
    pub ignore_dirs: HashSet<String>,
    pub bucket_thresholds: HashMap<String, f64>,
    pub category_overrides: HashMap<String, String>,
    pub max_depth: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            roots: vec![PathBuf::from("~/repos"), PathBuf::from("~/learning")],
            extra_paths: vec![],
            author_patterns: vec!["Jan.*Krag".to_string()],
            author_since: "3 years".to_string(),
            ignore_dirs: [
                "node_modules",
                ".worktrees",
                "vendor",
                ".venv",
                "venv",
                "target",
                "dist",
                "build",
                ".next",
                "Library",
                ".Trash",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            bucket_thresholds: [
                ("active".to_string(), 48.0),
                ("in_flight".to_string(), 336.0),
                ("stale".to_string(), 1440.0),
            ]
            .into_iter()
            .collect(),
            category_overrides: HashMap::new(),
            max_depth: 4,
        }
    }
}

#[derive(Debug)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for ConfigError {}

/// Default location: `~/.petridish/config.toml`.
pub fn default_path() -> PathBuf {
    todo!("R2: expand ~ + HOME, join .petridish/config.toml")
}

/// Load config from `path` (or defaults if missing). A malformed *file* (unparsable TOML)
/// is a `ConfigError`; a malformed *field* inside a parsable file degrades to that field's
/// default (warn-and-continue), matching `_coerce_*` behavior in config.py. `roots`/
/// `extra_paths` are `~`/env-expanded at load time, same as the defaults.
pub fn load_config(_path: &std::path::Path) -> Result<Config, ConfigError> {
    todo!("R2: read+parse TOML if present, per-field coercion with defaulting, path expansion")
}
