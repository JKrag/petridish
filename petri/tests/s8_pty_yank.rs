//! S8 acceptance gate, layer 3: the `y` yank action against the real binary
//! (petri/IDEAS.md `ACT-2`).
//!
//! Pressing `y` in normal mode copies the selected project's path to the
//! clipboard via `pbcopy`. `pbcopy` exists on macOS and not on Linux (CI runs
//! both), so this only asserts *some* notice appears after pressing `y` — never
//! a specific clipboard outcome. See lib.rs's `yank_selected_path` for why both
//! outcomes (a success notice, or a "could not copy" notice) are valid.

mod pty_support;
use pty_support::{Session, fixture_path};
use std::io::Write;
use std::time::Duration;

fn scratch_home(name: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("petri_s8_pty_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("scratch home must be creatable");
    home
}

fn settle(session: &mut Session) -> Vec<String> {
    session.screen_retry(
        90,
        40,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
    )
}

fn send(session: &mut Session, bytes: &[u8]) {
    session.writer.write_all(bytes).expect("write must succeed");
    session.writer.flush().expect("flush must succeed");
}

fn to_browser(home: &std::path::Path) -> Session {
    let mut session = Session::spawn_with_home(&fixture_path("loaded.json"), 90, 40, home);
    settle(&mut session);
    send(&mut session, b"\t");
    let screen = settle(&mut session);
    assert!(
        screen.iter().any(|r| r.contains("browser")),
        "expected to be on the Browser after Tab, got:\n{}",
        screen.join("\n")
    );
    session
}

#[test]
fn yank_produces_a_notice_on_every_platform() {
    // pbcopy exists on macOS and not on Linux (CI runs both), so this only
    // asserts SOME notice appears after pressing `y` — never a specific
    // clipboard outcome. See lib.rs's yank_selected_path for why both
    // outcomes are valid.
    let home = scratch_home("yank");
    let mut session = to_browser(&home);
    send(&mut session, b"y");
    let screen = settle(&mut session);
    assert!(
        screen.iter().any(|r| r.contains("clipboard")),
        "expected a clipboard-related notice after pressing y, got:\n{}",
        screen.join("\n")
    );
}
