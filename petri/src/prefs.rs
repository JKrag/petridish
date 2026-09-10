//! The preferences file (petri/SPEC.md §6): `~/.petridish/petri.toml`, owned
//! and written by `petri` alone — `swab` never reads it, and it is
//! deliberately not a `[petri]` section in `config.toml` (that would put two
//! writers on one file). Holds which Dashboard sections are collapsed, the
//! last active screen, and the tool choices for the first-run picker (which
//! external program each action id falls back to), so all three survive a
//! restart.
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
#[derive(Default)]
pub enum LastScreen {
    #[default]
    Dashboard,
    Browser,
}

/// Whether to draw the Browser's git segment with Nerd Font glyphs (issue #38).
///
/// **There is no way to ask a terminal what font it is using.** No escape sequence reports
/// it, and the obvious probe — print a glyph, query the cursor column — cannot tell success
/// from failure either: a missing Private-Use-Area glyph renders as a tofu box that still
/// occupies exactly one cell, so the measurement comes back identical either way. That is
/// why starship, lazygit and k9s all make this explicit configuration.
///
/// `Auto` is therefore a *heuristic*, and an honest one only because of font fallback: both
/// macOS (CoreText) and Linux (fontconfig) will render a glyph from some other installed
/// font when the terminal's own font lacks it. So "is a Nerd Font installed anywhere on this
/// machine" predicts "will this glyph appear" well enough to default to, and is overridable
/// in both directions for the cases where it guesses wrong — a remote tmux session being the
/// obvious one, where the fonts that matter are on the *other* machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum NerdFonts {
    /// Use them if a Nerd Font appears to be installed. The default.
    #[default]
    Auto,
    /// Always use them, whatever the probe says.
    Always,
    /// Never use them; the ASCII segment at every width.
    Never,
}

/// The persisted preferences shape. `#[serde(default)]` on every field so a
/// prefs file written by an older `petri` (schema drift) still parses.
///
/// `tools` is declared last as a readability convention, not a correctness
/// requirement: it keeps the scalars together above the `[tools]` table in the
/// written file, matching how a human would hand-edit it. (An earlier version
/// of this comment claimed `toml::to_string` would fail with `ValueAfterTable`
/// if a map preceded a scalar. That was true of the toml 0.5-era serializer;
/// it is measurably NOT true of the toml 0.8 / toml_edit backend this crate
/// pins, which emits a correct document from either order. Verified directly
/// rather than assumed.)
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Prefs {
    #[serde(default)]
    pub last_screen: LastScreen,
    #[serde(default = "default_collapsed")]
    pub collapsed: CollapsedState,
    /// Nerd Font glyphs in the Browser's git segment. See [`NerdFonts`] for why this
    /// cannot simply be detected.
    #[serde(default)]
    pub nerd_fonts: NerdFonts,
    /// The external program chosen for each action id (e.g. `"gitlog"` ->
    /// `"serie"`). Absent from an older prefs file, so `#[serde(default)]`.
    #[serde(default)]
    pub tools: std::collections::BTreeMap<String, String>,
}

impl Prefs {
    /// Resolve [`NerdFonts`] against the machine.
    ///
    /// `probe` is injected rather than called directly so this stays pure and the two
    /// non-`Auto` arms can be tested without touching a filesystem — and so a test can pin
    /// either answer for `Auto`. `exec::nerd_font_installed` is the production probe.
    pub fn use_nerd_fonts(&self, probe: &dyn Fn() -> bool) -> bool {
        match self.nerd_fonts {
            NerdFonts::Always => true,
            NerdFonts::Never => false,
            NerdFonts::Auto => probe(),
        }
    }
}

fn default_collapsed() -> CollapsedState {
    [false, false, true, true]
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            last_screen: LastScreen::default(),
            collapsed: default_collapsed(),
            nerd_fonts: NerdFonts::default(),
            tools: std::collections::BTreeMap::new(),
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
            eprintln!(
                "petri S7: preferences file at {:?} missing or unreadable ({e}), using defaults",
                path
            );
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
    std::fs::create_dir_all(
        path.parent()
            .ok_or_else(|| std::io::Error::other(format!("path has no parent: {path:?}")))?,
    )?;
    let text = toml::to_string(prefs).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to serialize prefs: {e}"),
        )
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
    use super::{LastScreen, NerdFonts, Prefs};

    #[test]
    fn nerd_fonts_always_and_never_ignore_the_probe() {
        // The override has to beat the heuristic in BOTH directions — a remote tmux session
        // is the case that motivates it, where the fonts that matter are on another machine
        // entirely and the local probe is answering the wrong question.
        let always = Prefs {
            nerd_fonts: NerdFonts::Always,
            ..Prefs::default()
        };
        assert!(
            always.use_nerd_fonts(&|| false),
            "Always beats a false probe"
        );

        let never = Prefs {
            nerd_fonts: NerdFonts::Never,
            ..Prefs::default()
        };
        assert!(!never.use_nerd_fonts(&|| true), "Never beats a true probe");
    }

    #[test]
    fn nerd_fonts_auto_follows_the_probe() {
        let prefs = Prefs::default();
        assert_eq!(prefs.nerd_fonts, NerdFonts::Auto, "Auto is the default");
        assert!(prefs.use_nerd_fonts(&|| true));
        assert!(!prefs.use_nerd_fonts(&|| false));
    }

    #[test]
    fn a_prefs_file_without_nerd_fonts_still_parses() {
        // Schema drift: every field is `#[serde(default)]` precisely so an older file keeps
        // working, and this is the newest field.
        let older = r#"
            last_screen = "browser"
            collapsed = [false, false, true, true]
        "#;
        let prefs: Prefs = toml::from_str(older).expect("older prefs file must parse");
        assert_eq!(prefs.nerd_fonts, NerdFonts::Auto);
        assert_eq!(prefs.last_screen, LastScreen::Browser);
    }

    #[test]
    fn nerd_fonts_round_trips_through_toml() {
        let prefs = Prefs {
            nerd_fonts: NerdFonts::Never,
            ..Prefs::default()
        };
        let text = toml::to_string(&prefs).expect("must serialise");
        let back: Prefs = toml::from_str(&text).expect("must parse back");
        assert_eq!(back.nerd_fonts, NerdFonts::Never, "wrote: {text}");
    }

    // Pure-state contract tests live in `petri/tests/s7_prefs.rs` (the
    // orchestrator-authored acceptance gate) — nothing here yet.
}
