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
/// -> `Prefs::default()` PLUS a warning to stderr — never a panic, never a
/// refusal to start (petri/SPEC.md §6).
pub fn load(_path: &Path) -> Prefs {
    todo!("S7: prefs::load")
}

/// Save `prefs` to `path` atomically (temp file in the same directory, then
/// rename) — same pattern as `petridish_core::schema::write_atomic`. Creates
/// the parent directory if missing.
pub fn save(_path: &Path, _prefs: &Prefs) -> std::io::Result<()> {
    todo!("S7: prefs::save")
}

#[cfg(test)]
mod tests {
    // Pure-state contract tests live in `petri/tests/s7_prefs.rs` (the
    // orchestrator-authored acceptance gate) — nothing here yet.
}
