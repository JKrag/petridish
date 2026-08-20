//! S7 acceptance gate, layer 1 (petri/SPEC.md §6/§8, ADR-0003) — protected,
//! authored by the orchestrator, not the delegate. Pure-state contract tests
//! for `prefs::load`/`prefs::save`, no terminal/rendering involved.
//!
//! petri/SPEC.md §6 is explicit and testable: "A missing file means
//! defaults. A corrupt or unparseable file means defaults plus a warning —
//! never a crash, and never a refusal to start. There is a test for this."
//! The "never a crash" half is covered here (pure-state); the "starts
//! normally end-to-end" half needs the real binary and lives in
//! `s7_pty.rs`'s `corrupt_petri_toml_does_not_prevent_startup` test.
//!
//! `prefs::load`/`prefs::save`'s `todo!()` bodies mean every test below
//! FAILS (panics) rather than errors — confirmed before delegating S7.

use petri::dashboard::CollapsedState;
use petri::prefs::{self, LastScreen, Prefs};
use std::path::PathBuf;

/// A fresh scratch path under the OS tmpdir, unique per test (via the test's
/// own name plus a random suffix) so parallel test runs never collide.
fn scratch_path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("petri_s7_prefs_test_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir must be creatable");
    dir.join("petri.toml")
}

#[test]
fn missing_file_returns_defaults() {
    let path = scratch_path("missing_file_returns_defaults");
    assert!(!path.exists(), "test precondition: scratch path must not exist yet");
    let prefs = prefs::load(&path);
    assert_eq!(prefs, Prefs::default());
    assert_eq!(prefs.last_screen, LastScreen::Dashboard);
    assert_eq!(prefs.collapsed, [false, false, true, true]);
}

#[test]
fn corrupt_file_returns_defaults_not_a_panic() {
    let path = scratch_path("corrupt_file_returns_defaults_not_a_panic");
    std::fs::write(&path, b"this is not valid toml { [[[ ===").expect("write garbage must succeed");
    // Must not panic (petri/SPEC.md §6: "never a crash") and must fall back
    // to defaults, not a partially-parsed or zeroed struct.
    let prefs = prefs::load(&path);
    assert_eq!(prefs, Prefs::default());
}

#[test]
fn empty_file_returns_defaults_not_a_panic() {
    // A zero-byte file is a distinct edge case from garbage bytes (some TOML
    // parsers treat an empty document as valid-but-empty, which would parse
    // successfully into a struct missing every field — `#[serde(default)]`
    // on every field must cover this, not just outright parse failure).
    let path = scratch_path("empty_file_returns_defaults_not_a_panic");
    std::fs::write(&path, b"").expect("write empty file must succeed");
    let prefs = prefs::load(&path);
    assert_eq!(prefs, Prefs::default());
}

#[test]
fn roundtrip_save_then_load_preserves_non_default_values() {
    let path = scratch_path("roundtrip_save_then_load_preserves_non_default_values");
    let written = Prefs {
        last_screen: LastScreen::Browser,
        collapsed: [true, true, false, false],
    };
    prefs::save(&path, &written).expect("save must succeed");
    let read_back = prefs::load(&path);
    assert_eq!(read_back, written);
}

#[test]
fn save_creates_missing_parent_directory() {
    let dir = std::env::temp_dir().join(format!("petri_s7_prefs_test_nested_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("nested").join("deeper").join("petri.toml");
    assert!(!dir.exists(), "test precondition: parent dir tree must not exist yet");

    prefs::save(&path, &Prefs::default()).expect("save must create missing parent dirs");
    assert!(path.exists(), "target file must exist after save");
}

#[test]
fn save_leaves_no_tmp_file_behind() {
    let path = scratch_path("save_leaves_no_tmp_file_behind");
    prefs::save(&path, &Prefs::default()).expect("save must succeed");
    let parent = path.parent().expect("scratch path must have a parent");
    let leftover_tmp: Vec<_> = std::fs::read_dir(parent)
        .expect("scratch dir must be readable")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(leftover_tmp.is_empty(), "no .tmp file should remain after a successful save, found: {leftover_tmp:?}");
}

#[test]
fn save_overwrite_second_call_replaces_the_first() {
    let path = scratch_path("save_overwrite_second_call_replaces_the_first");
    prefs::save(&path, &Prefs { last_screen: LastScreen::Dashboard, collapsed: [false, false, true, true] })
        .expect("first save must succeed");
    prefs::save(&path, &Prefs { last_screen: LastScreen::Browser, collapsed: [true, false, true, false] })
        .expect("second save must succeed");
    let read_back = prefs::load(&path);
    assert_eq!(read_back.last_screen, LastScreen::Browser);
    assert_eq!(read_back.collapsed, [true, false, true, false]);
}

#[test]
fn default_prefs_path_is_under_petridish_home_dir() {
    // Mirrors `default_state_path`'s convention (`lib.rs`) — same sibling
    // directory, different filename.
    let path = prefs::default_prefs_path();
    assert!(path.ends_with("petri.toml"), "got: {path:?}");
    assert!(path.to_string_lossy().contains(".petridish"), "got: {path:?}");
}

/// Not a real gate assertion, just documents the type contract other tests
/// rely on: `CollapsedState` and `Prefs::collapsed` must be the same shape as
/// `DashboardState`'s own field, so a loaded prefs file can seed
/// `DashboardState::with_collapsed` directly without conversion.
#[test]
fn collapsed_state_type_matches_dashboard_state() {
    let c: CollapsedState = [true, false, true, false];
    let p = Prefs { last_screen: LastScreen::Dashboard, collapsed: c };
    assert_eq!(p.collapsed, c);
}
