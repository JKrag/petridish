//! S8 acceptance gate, layer 3: the `?` help popup (`petri/IDEAS.md` ACT-2).
//!
//! The popup is a modal that consumes every keystroke while open — any key
//! closes it, and nothing falls through to a normal-mode binding. This proves
//! both halves: pressing `?` opens it (the title appears), and a subsequent
//! `j` — which in normal mode would move the selection — instead closes it,
//! leaving no trace of the popup on screen.

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
fn help_popup_opens_and_closes_on_any_key() {
    let home = scratch_home("help");
    let mut session = to_browser(&home);
    send(&mut session, b"?");
    let opened = settle(&mut session);
    assert!(
        opened.iter().any(|r| r.contains("help")),
        "expected the help popup title after pressing ?, got:\n{}",
        opened.join("\n")
    );
    // Any key closes it, including one with its own normal-mode binding —
    // proving the popup consumed the keystroke rather than letting it fall
    // through to e.g. move_selection.
    send(&mut session, b"j");
    let closed = settle(&mut session);
    assert!(
        !closed.iter().any(|r| r.contains("any key closes")),
        "expected the help popup to be gone after any keypress, got:\n{}",
        closed.join("\n")
    );
}
