//! `~/.petridish/config.toml` loader. Mirrors `src/petridish/config.py`.
//! Entirely optional file — every field has a default; a missing file is valid,
//! a malformed one degrades field-by-field (never aborts the whole load).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Literal marker string appended to hook `command` entries in `~/.claude/settings.json`;
/// shared with `swab doctor`'s hook-detection.
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

/// `~/.petridish/config.toml` under the given home directory. Kept separate from
/// `default_path()` so tests can exercise the real path-composition logic against a tmp dir
/// without mutating the process-wide `HOME` env var (which is `unsafe` to set under the
/// 2024 edition and racy under parallel tests regardless).
pub fn for_home(home: &std::path::Path) -> PathBuf {
    home.join(".petridish").join("config.toml")
}

/// Default location: `~/.petridish/config.toml`, reading `$HOME` from the environment.
pub fn default_path() -> PathBuf {
    for_home(&PathBuf::from(std::env::var("HOME").expect("HOME must be set")))
}

/// A single-string value from a TOML table entry — `None` when the key is missing or
/// not a string. Used to fall back to defaults without failing the whole load.
fn as_string(value: &toml::Value) -> Option<String> {
    value.as_str().map(String::from)
}

/// A list of strings — for `roots`, `extra_paths`, `author_patterns`, and
/// `ignore_dirs`. Every entry must be a TOML string; otherwise we return
/// `None` so the caller can fall back to defaults.
fn as_string_list(table: &toml::value::Table, key: &str) -> Option<Vec<String>> {
    let array = table.get(key)?.as_array()?;
    if array.is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::with_capacity(array.len());
    for v in array {
        if let Some(s) = v.as_str() {
            out.push(s.to_string());
        } else {
            return None;
        }
    }
    Some(out)
}

/// Path-string list (`roots` / `extra_paths`). Same shape as [`as_string_list`]
/// but expands `~`/env prefixes and returns `PathBuf`s directly — so the caller
/// doesn't have to remember to expand after reading a path field.
fn as_path_list(table: &toml::value::Table, key: &str) -> Option<Vec<PathBuf>> {
    let items = as_string_list(table, key)?;
    if items.is_empty() {
        return Some(Vec::new());
    }
    let home = std::env::var("HOME").unwrap_or_default();
    Some(items.iter().map(|s| PathBuf::from(expand_path(s, &home))).collect())
}

/// A `{string: number}` table (`bucket_thresholds`), coerced per-key against `defaults`
/// rather than all-or-nothing. Mirrors `config.py::_coerce_durations` exactly: a *missing*
/// key falls back to that key's own default, and a *non-numeric* value for a present key
/// also falls back to that key's own default — only a key present in `defaults` is ever
/// emitted (an extra user-supplied key not in `defaults` is silently dropped, matching
/// Python's `for key, default_value in defaults.items()` loop bound). Returns `None` only
/// when the whole field isn't a table at all (missing key, or present but not a table),
/// which leaves the caller's already-defaulted `Config::default()` value untouched — same
/// effect as Python's "not isinstance(user, dict)" branch, which also falls through to
/// full defaults.
///
/// Found via a gap audit: an earlier version returned only the keys present in the file,
/// so `[bucket_thresholds]\nactive = 72` would leave `in_flight`/`stale` missing from the
/// map entirely instead of defaulted.
fn as_float_table(
    table: &toml::value::Table,
    key: &str,
    defaults: &HashMap<String, f64>,
) -> Option<HashMap<String, f64>> {
    let tbl = table.get(key)?.as_table()?;
    let mut out = HashMap::with_capacity(defaults.len());
    for (k, default_value) in defaults {
        let v = match tbl.get(k) {
            Some(v) => v,
            None => {
                out.insert(k.clone(), *default_value);
                continue;
            }
        };
        let f = match (v.as_float(), v.as_integer()) {
            (Some(f), _) => f,
            (_, Some(i)) => i as f64,
            _ => *default_value,
        };
        out.insert(k.clone(), f);
    }
    Some(out)
}

/// A `{string: string}` table (`category_overrides`), filtered per-key rather than
/// all-or-nothing. Mirrors `config.py::load_config`'s category_overrides handling exactly:
/// `{str(k): v for k, v in raw_overrides.items() if isinstance(v, str)}` — a non-string
/// value for one key drops just that key, the rest of the table survives. Returns `None`
/// only when the whole field isn't a table at all, matching Python's `else: {}` branch
/// (falls through to the caller's already-empty `Config::default()` value).
///
/// Found via a gap audit: an earlier version bailed the whole table to `None` on the
/// first non-string value.
fn as_str_table(
    table: &toml::value::Table,
    key: &str,
) -> Option<HashMap<String, String>> {
    let tbl = table.get(key)?.as_table()?;
    let mut out = HashMap::with_capacity(tbl.len());
    for (k, v) in tbl {
        if let Some(s) = v.as_str() {
            out.insert(k.clone(), s.to_string());
        }
    }
    Some(out)
}

/// `~` and `$VAR` expansion for path strings — mirrors the `_expand_path` helper in
/// `src/petridish/config.py`. Per the contract, this applies to file-provided values
/// *and* defaults, since `load_config` seeds from `Config::default()` and then overrides
/// in place (so the same expansion path handles both).
fn expand_path(s: &str, home: &str) -> String {
    let s = if s == "~" {
        home.to_string()
    } else if let Some(rest) = s.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else {
        s.to_string()
    };
    // `$VAR` expansion — only a leading `$` matters; we don't try to be a full shell.
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            let mut name = String::new();
            while let Some(ch) = chars.peek() {
                if ch.is_alphanumeric() || *ch == '_' {
                    name.push(*ch);
                    chars.next();
                } else {
                    break;
                }
            }
            if let Ok(val) = std::env::var(&name) {
                out.push_str(&val);
            } else {
                // Unknown var: leave `$NAME` untouched rather than silently stripping it.
                out.push('$');
                out.push_str(&name);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Load config from `path` (or defaults if missing). A malformed *file* (unparsable TOML)
/// is a `ConfigError`; a malformed *field* inside a parsable file degrades to that
/// field's default (warn-and-continue), matching `_coerce_*` behavior in `config.py`.
/// `roots`/`extra_paths` are `~`/env-expanded at load time regardless of source — both
/// file-provided AND seeded-default values go through `expand_path` before this function
/// returns (see the unconditional pass at the bottom), matching the contract that overrides
/// get the same treatment as defaults. `Config::default()` itself intentionally keeps the
/// raw `~/...` strings so callers constructing a `Config` directly (bypassing this loader,
/// e.g. in tests) aren't silently mutated — expansion is this function's job, not the
/// `Default` impl's.
pub fn load_config(path: &std::path::Path) -> Result<Config, ConfigError> {
    let mut cfg = Config::default();

    // A missing config file is valid — defaults apply. Never raise here. Still falls
    // through to the unconditional expansion pass below, not an early return, so these
    // defaults come out expanded too.
    if !path.exists() {
        return Ok(expand_config_paths(cfg));
    }

    let raw = std::fs::read_to_string(path)
        .map_err(|e| ConfigError(format!("cannot read config {:?}: {e}", path)))?;
    let table: toml::Table = toml::from_str(&raw)
        .map_err(|e| ConfigError(format!("malformed config {:?}: {e}", path)))?;

    // Per-field override: parse into the typed shape we want, fall back to the seeded
    // default on any type mismatch. Defaults are applied first so a bad value simply
    // leaves the field alone. Path fields go through `as_path_list` which expands
    // `~`/env prefixes into absolute `PathBuf`s; non-path string lists keep raw
    // strings (no expansion needed).
    if let Some(items) = as_path_list(&table, "roots") {
        cfg.roots = items;
    }
    if let Some(items) = as_path_list(&table, "extra_paths") {
        cfg.extra_paths = items;
    }
    if let Some(items) = as_string_list(&table, "author_patterns") {
        cfg.author_patterns = items;
    }
    if let Some(v) = table.get("author_since").and_then(as_string) {
        cfg.author_since = v;
    }
    if let Some(items) = as_string_list(&table, "ignore_dirs") {
        cfg.ignore_dirs = items.into_iter().collect();
    }
    if let Some(v) = as_float_table(&table, "bucket_thresholds", &Config::default().bucket_thresholds) {
        cfg.bucket_thresholds = v;
    }
    if let Some(v) = as_str_table(&table, "category_overrides") {
        cfg.category_overrides = v;
    }
    if let Some(depth) = table.get("max_depth").and_then(|v| v.as_integer()) {
        // Booleans are rejected: `as_integer()` returns None for bools.
        if depth >= 0 && depth <= std::u32::MAX as i64 {
            cfg.max_depth = depth as u32;
        }
    }

    Ok(expand_config_paths(cfg))
}

/// Unconditionally `~`/env-expands `cfg.roots`/`cfg.extra_paths`, whether they came from
/// the file (already expanded by `as_path_list`, so this is a harmless no-op re-pass) or
/// fell through untouched from `Config::default()`'s raw `~/...` seed strings (the case
/// this function exists to fix — see `load_config`'s doc comment).
fn expand_config_paths(mut cfg: Config) -> Config {
    let home = std::env::var("HOME").unwrap_or_default();
    let expand_one = |p: &PathBuf| PathBuf::from(expand_path(&p.to_string_lossy(), &home));
    cfg.roots = cfg.roots.iter().map(expand_one).collect();
    cfg.extra_paths = cfg.extra_paths.iter().map(expand_one).collect();
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Test helper: write `contents` to a temp file and return the path.
    fn with_tmp(name: &str, contents: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("swab_rs_test_config");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }

    /// Test 1: nonexistent path -> Ok(defaults), EXPANDED — `load_config`'s output is
    /// never equal to raw `Config::default()` for `roots`, since expansion is this
    /// function's contract regardless of whether the file exists (see next test for the
    /// regression this guards).
    #[test]
    fn load_missing_path_returns_defaults() {
        let path = std::path::PathBuf::from("/tmp/nonexistent_swab_test_12345.toml");
        let cfg = load_config(&path).expect("missing path must not error");
        assert_eq!(cfg.max_depth, Config::default().max_depth);
        assert_eq!(cfg.author_patterns, Config::default().author_patterns);
        for root in &cfg.roots {
            assert!(
                !root.to_string_lossy().starts_with('~'),
                "default roots must be expanded too, got {root:?}"
            );
        }
    }

    /// Regression test: a config file that does NOT mention `roots` at all must still
    /// return EXPANDED default roots, not the raw `~/repos` seed string. Caught by manual
    /// review after the initial delegated implementation only expanded file-provided
    /// values and left `Config::default()`'s roots untouched when omitted from the file —
    /// every downstream `discover()` call against that config would have silently found
    /// zero projects, since `PathBuf::from("~/repos")` is not a real filesystem path.
    #[test]
    fn omitted_roots_in_file_still_expand_the_defaults() {
        let path = with_tmp("no_roots.toml", "max_depth = 9\n");
        let cfg = load_config(&path).expect("must parse");
        assert_eq!(cfg.max_depth, 9);
        assert!(!cfg.roots.is_empty(), "defaults must still populate roots");
        for root in &cfg.roots {
            assert!(
                !root.to_string_lossy().starts_with('~'),
                "expected expanded default root, got {root:?}"
            );
        }
    }

    /// Test 2: a valid TOML overriding `max_depth` changes only that field.
    #[test]
    fn valid_toml_overrides_max_depth_only() {
        // Top-level key — `toml::Table` flattens sections at parse time, so the
        // override must appear directly under the outer table.
        let path = with_tmp("valid.toml", "max_depth = 7\n");
        let cfg = load_config(&path).expect("parse should succeed");
        assert_eq!(cfg.max_depth, 7);
        // All other fields remain at defaults — roots come back EXPANDED, per
        // load_config's contract (see `omitted_roots_in_file_still_expand_the_defaults`).
        for root in &cfg.roots {
            assert!(!root.to_string_lossy().starts_with('~'));
        }
        assert_eq!(cfg.extra_paths, Vec::<PathBuf>::new());
        assert_eq!(cfg.author_patterns, Config::default().author_patterns);
        assert_eq!(cfg.author_since, "3 years");
        assert_eq!(cfg.ignore_dirs, Config::default().ignore_dirs);
        assert_eq!(cfg.bucket_thresholds, Config::default().bucket_thresholds);
        assert_eq!(cfg.category_overrides, Config::default().category_overrides);
    }

    /// Test 3: roots as a bare string (wrong type) falls back to default — does NOT
    /// error, and the whole file still loads.
    #[test]
    fn roots_string_falls_back_to_default() {
        let path = with_tmp("roots_bad.toml", "roots = \"~/repos\"\n");
        let cfg = load_config(&path).expect("type mismatch must degrade to default");
        // Falls back to the default roots, expanded (same contract as the missing-file case).
        for root in &cfg.roots {
            assert!(!root.to_string_lossy().starts_with('~'));
        }
    }

    /// Test 4: syntactically broken TOML -> ConfigError (file-level error).
    #[test]
    fn malformed_toml_returns_error() {
        let path = with_tmp("broken.toml", "roots = [\"a\", \"b\"\n"); // unterminated array
        let err = load_config(&path).expect_err("broken TOML must error");
        assert!(!err.0.is_empty(), "error message must name the problem");
    }

    /// Test 5: `~` in roots is expanded to an absolute path.
    #[test]
    fn tilde_is_expanded() {
        let home = std::env::var("HOME").expect("HOME must be set for this test");
        let path = with_tmp("tilde.toml", r#"roots = ["~/repos", "~/learning"]"#);
        let cfg = load_config(&path).expect("expand must succeed");

        // Every root must be absolute (no leading `~`) — default-expansion guard.
        for root in &cfg.roots {
            assert!(
                !root.to_string_lossy().starts_with('~'),
                "expected absolute path, got {:?}", root
            );
        }
        // And order/contents match the input (after expansion).
        assert_eq!(
            cfg.roots,
            vec![
                PathBuf::from(format!("{home}/repos")),
                PathBuf::from(format!("{home}/learning")),
            ]
        );
    }

    /// Test 6: for_home composes paths exactly — exercise `for_home` directly.
    #[test]
    fn for_home_composes_path() {
        let expected = std::path::PathBuf::from("/some/tmp/home/.petridish/config.toml");
        assert_eq!(for_home(std::path::Path::new("/some/tmp/home")), expected);
    }

    /// Test 7: bucket_thresholds (table with numbers) and category_overrides (table of
    /// strings) both load from a TOML table. Provided keys override defaults; missing
    /// keys fall back to the seeded defaults.
    #[test]
    fn table_fields_load_correctly() {
        let toml_text = r#"
[bucket_thresholds]
active = 72.0
in_flight = 336.0
stale = 1440.0

[category_overrides]
"*.js" = "javascript"
"*.py" = "python"
"#;
        let path = with_tmp("tables.toml", toml_text);
        let cfg = load_config(&path).expect("table fields must parse");

        assert_eq!(cfg.bucket_thresholds.get("active"), Some(&72.0));
        // Provided + seeded defaults both present.
        assert_eq!(cfg.bucket_thresholds.get("in_flight"), Some(&336.0));
        assert_eq!(cfg.bucket_thresholds.get("stale"), Some(&1440.0));
        // Missing key gets seeded default.
        assert_eq!(cfg.bucket_thresholds.get("unused"), None);

        assert_eq!(cfg.category_overrides.get("*.js").unwrap(), "javascript");
        assert_eq!(cfg.category_overrides.get("*.py").unwrap(), "python");
    }

    /// Regression: found via a gap audit against `config.py::_coerce_durations`. A partial
    /// `[bucket_thresholds]` override (only `active` set) must still produce a map with all
    /// three keys — `in_flight`/`stale` defaulted, not absent. An earlier version only
    /// emitted the keys present in the file.
    #[test]
    fn bucket_thresholds_partial_override_merges_with_defaults() {
        let toml_text = "[bucket_thresholds]\nactive = 72.0\n";
        let path = with_tmp("partial_thresholds.toml", toml_text);
        let cfg = load_config(&path).expect("partial table must parse");

        assert_eq!(cfg.bucket_thresholds.get("active"), Some(&72.0));
        assert_eq!(
            cfg.bucket_thresholds.get("in_flight"),
            Config::default().bucket_thresholds.get("in_flight"),
            "in_flight must default, not be absent, when only active is overridden"
        );
        assert_eq!(
            cfg.bucket_thresholds.get("stale"),
            Config::default().bucket_thresholds.get("stale"),
            "stale must default, not be absent, when only active is overridden"
        );
    }

    /// Regression: found via a gap audit against `config.py::load_config`'s
    /// category_overrides handling (`{k: v for k, v in raw.items() if isinstance(v, str)}`).
    /// One bad (non-string) value in the table must drop only that key, not the whole
    /// field. An earlier version fell back to `{}` (empty) on the first bad value.
    #[test]
    fn category_overrides_drops_only_the_bad_key() {
        let toml_text = "[category_overrides]\n\"*.js\" = \"javascript\"\n\"*.bad\" = 42\n\"*.py\" = \"python\"\n";
        let path = with_tmp("partial_overrides.toml", toml_text);
        let cfg = load_config(&path).expect("table with one bad value must still parse");

        assert_eq!(cfg.category_overrides.get("*.js").unwrap(), "javascript");
        assert_eq!(cfg.category_overrides.get("*.py").unwrap(), "python");
        assert_eq!(
            cfg.category_overrides.get("*.bad"), None,
            "the non-string value's key must be dropped, not crash the whole table"
        );
        assert_eq!(cfg.category_overrides.len(), 2, "the two good keys must survive");
    }

    /// Sanity check: env-var prefix on a path string is expanded too.
    #[test]
    fn env_var_prefix_is_expanded() {
        let path = with_tmp("envvar.toml", "roots = [\"$HOME/repos\"]");
        let cfg = load_config(&path).expect("env-expand must succeed");
        assert_eq!(cfg.roots.len(), 1);
        let s = cfg.roots[0].to_string_lossy();
        assert!(!s.starts_with('$'), "env var should have been expanded: {s:?}");
    }
}
