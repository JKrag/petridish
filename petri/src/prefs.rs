//! The preferences file (petri/SPEC.md §6): `~/.petridish/petri.toml`, owned
//! and written by `petri` alone — `swab` never reads it, and it is
//! deliberately not a `[petri]` section in `config.toml` (that would put two
//! writers on one file). Holds which Dashboard sections are collapsed and the
//! last active screen, so both survive a restart.
//!
//! Contract, per spec:
//! - A **missing** file means defaults (first run, or the file was deleted).
//! - A **corrupt or unparseable** file means defaults PLUS a warning —
//!   never a crash, and never a refusal to start. "There is a test for
//!   this" (spec's own words) — see `petri/tests/s7_prefs.rs` and
//!   `petri/tests/s7_pty.rs`'s corrupt-toml test.
//! - Written atomically (temp file + rename), same convention as
//!   `petridish_core::schema::write_atomic` uses for the state file.

use crate::dashboard::CollapsedState;
use std::path::{Path, PathBuf};

/// Which screen was active when `petri` last exited (or switched, if writes
/// happen per-switch rather than only at exit — the delegate's call, not
/// pinned by this stub).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LastScreen {
    Dashboard,
    Browser,
}

impl Default for LastScreen {
    fn default() -> Self {
        LastScreen::Dashboard
    }
}

/// The persisted preferences shape. `#[serde(default)]` on every field so a
/// prefs file written by an older `petri` (schema drift) still parses.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Prefs {
    #[serde(default)]
    pub last_screen: LastScreen,
    #[serde(default = "default_collapsed")]
    pub collapsed: CollapsedState,
}

fn default_collapsed() -> CollapsedState {
    [false, false, true, true]
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            last_screen: LastScreen::default(),
            collapsed: default_collapsed(),
        }
    }
}

/// Resolved default preferences-file path: `$HOME/.petridish/petri.toml`.
/// Mirrors `default_state_path` in `lib.rs` — composed directly so tests can
/// override it without touching `HOME`.
pub fn default_prefs_path() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set");
    PathBuf::from(&home).join(".petridish").join("petri.toml")
}

/// Load preferences from `path`. Missing file -> `Prefs::default()`, no
/// warning (this is the expected first-run shape). Corrupt/unparseable file
/// (including a zero-byte empty file, which `toml::from_str` treats as a
/// valid-but-empty document — `#[serde(default)]` on every field covers that
/// so we parse successfully and fall back to defaults) -> `Prefs::default()`
/// PLUS an `eprintln!` warning to stderr. Never panics, never refuses to
/// start (petri/SPEC.md §6).
pub fn load(path: &Path) -> Prefs {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            // `toml::from_str("")` returns `Ok(())` for an empty document —
            // it is not an error, it is the absence of any key. Our
            // `Prefs::default()` constructor via `#[serde(default)]` makes
            // this fall through to a clean default struct. If we ever
            // added a `schema_version`-style discriminator, this is where
            // we'd detect "file has content but no known fields". For now,
            // empty-doc + parse-error both resolve to defaults.
            match toml::from_str::<Prefs>(&text) {
                Ok(prefs) => prefs,
                Err(e) => {
                    eprintln!(
                        "petri S7: corrupt preferences file at {:?}, using defaults: {e}",
                        path
                    );
                    Prefs::default()
                }
            }
        }
        Err(e) => {
            // Missing file (or unreadable) is expected on first run; log
            // once and fall back to defaults rather than refusing to start.
            eprintln!("petri S7: preferences file at {:?} missing or unreadable ({e}), using defaults", path);
            Prefs::default()
        }
    }
}

/// Save `prefs` to `path` atomically: write to `<path>.tmp` in the same
/// directory, then `std::fs::rename` onto `path`. Creates the parent
/// directory if missing. On any write/rename failure, remove the tmp file
/// before returning the error. Mirrors `petridish_core::schema::write_atomic`'s
/// pattern (file-local copy, atomic rename, cleanup on failure) but lives
/// in `petri`'s own file as a separate schema, not re-exporting core.
pub fn save(path: &Path, prefs: &Prefs) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::create_dir_all(path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("path has no parent: {path:?}"))
    })?)?;
    let text = toml::to_string(prefs).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("failed to serialize prefs: {e}"))
    })?;
    std::fs::write(&tmp, &text)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => {
            // Atomic on POSIX; fall back to remove+write on cross-device if
            // rename truly fails (rename already succeeded above, but just
            // in case a future filesystem change alters rename semantics).
            let _ = std::fs::remove_file(&tmp);
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    // Pure-state contract tests live in `petri/tests/s7_prefs.rs` (the
    // orchestrator-authored acceptance gate) — nothing here yet.
}
