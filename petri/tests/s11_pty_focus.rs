//! `SURF-8` mount #30, layer 3 (`petri/SPEC.md` §8, ADR-0003) — the Dashboard's focus
//! popup driven through a real pseudo-terminal against the compiled binary.
//!
//! **Added attended, after Phase D**, which is `PLAN-focus-panel.md` §9's explicit rule:
//! an unattended loop cannot tell a PTY flake from a defect, so it either halts on a false
//! failure or learns to ignore the layer.
//!
//! Five of this file's original six tests were exactly the "conceptually pure-state, no
//! terminal-only property" shape the #48 audit named — proving `lib.rs` routes
//! `Space`/`Esc` into `press_space`/`close_focus`, not a re-assertion of the contract
//! `s11_focus_mount.rs` already pins as pure state. They stayed here only because
//! `poll_loop` had nowhere else to put them; issue #61 opened that seam
//! (`handle_key<B: Backend>`), and all five moved to `s61_key_dispatch.rs` against
//! `ratatui::backend::TestBackend`: `space_on_a_project_row_opens_the_focus_popup`,
//! `esc_closes_the_focus_popup`, `space_twice_on_a_row_closes_what_it_opened`,
//! `space_on_a_header_still_collapses_the_section`, and
//! `a_small_terminal_renders_the_panel_full_screen_instead_of_a_popup`.
//!
//! What remains is the one test that is genuinely terminal-only: whether an overlay
//! that opens and closes leaves the terminal in a state `q` can still exit cleanly from
//! — raw mode, the alternate screen, the event loop itself not wedging. No `TestBackend`
//! state test can see that class of failure.

mod pty_support;
use pty_support::{Session, fixture_path};
use std::io::Write;
use std::time::Duration;

const COLS: u16 = 80;
const ROWS: u16 = 40;

/// A scratch `HOME` per test, so a test that presses `Space` can never write through to
/// the real `~/.petridish/petri.toml` — `s7_pty.rs` records that isolation bug being
/// found the hard way, against a developer's actual preferences file.
fn scratch_home(tag: &str) -> std::path::PathBuf {
    let home =
        std::env::temp_dir().join(format!("petri_s11_pty_{tag}_home_{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("scratch home dir must be creatable");
    home
}

fn settled(session: &mut Session, cols: u16, rows: u16) -> Vec<String> {
    session.screen_retry(
        cols,
        rows,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
    )
}

#[test]
fn q_still_exits_zero_after_the_focus_gesture() {
    // The lifecycle check: an overlay that leaves the terminal in raw mode or wedges the
    // loop is the failure this layer exists to catch, and no state test can see it.
    let home = scratch_home("quit");
    let mut session = Session::spawn_with_home(&fixture_path("loaded.json"), COLS, ROWS, &home);
    let _ = settled(&mut session, COLS, ROWS);

    for keys in [&b"j"[..], b" ", b"j", b"k", &[0x1b], b" "] {
        session
            .writer
            .write_all(keys)
            .unwrap_or_else(|e| panic!("write {keys:?} must succeed: {e}"));
        let _ = session.settle(Duration::from_millis(500), Duration::from_millis(150));
        assert!(
            session.child.try_wait().ok().flatten().is_none(),
            "petri must still be alive after sending {keys:?}"
        );
    }

    session.writer.write_all(b"q").expect("write q");
    let status = session.wait_with_timeout(Duration::from_secs(5));
    assert_eq!(
        status.exit_code(),
        0,
        "'q' must exit 0 after a full open/navigate/close focus-popup gesture"
    );
}
