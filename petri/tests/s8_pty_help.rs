//! S8 acceptance gate, layer 3: the `?` help popup (`petri/IDEAS.md` ACT-2).
//!
//! The popup is a modal that consumes every keystroke while open — any key
//! closes it, and nothing falls through to a normal-mode binding. This proves
//! both halves: pressing `?` opens it (the title appears), and a subsequent
//! `j` — which in normal mode would move the selection — instead closes it,
//! leaving no trace of the popup on screen.

mod pty_support;
use pty_support::{BROWSER_HEADER, DASHBOARD_HEADER, Session, fixture_path};
use std::io::Write;
use std::time::Duration;

fn scratch_home(name: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("petri_s8_pty_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("scratch home must be creatable");
    home
}

/// Settle until the grid shows `needle`. Every post-keystroke assertion goes through this
/// or `settle_until_gone`, never an unconditional settle — see `CLAUDE.md`'s PTY rules.
fn settle_until(session: &mut Session, needle: &'static str) -> Vec<String> {
    session.screen_until(
        90,
        40,
        Duration::from_secs(5),
        Duration::from_millis(300),
        10,
        |grid| grid.iter().any(|r| r.contains(needle)),
    )
}

/// Settle until `needle` is gone — for asserting the popup closed.
fn settle_until_gone(session: &mut Session, needle: &'static str) -> Vec<String> {
    session.screen_until(
        90,
        40,
        Duration::from_secs(5),
        Duration::from_millis(300),
        10,
        |grid| !grid.iter().any(|r| r.contains(needle)),
    )
}

fn send(session: &mut Session, bytes: &[u8]) {
    session.writer.write_all(bytes).expect("write must succeed");
    session.writer.flush().expect("flush must succeed");
}

/// Spawn on the Dashboard, wait until petri is genuinely there, then `Tab` to the Browser.
///
/// Neither wait is the obvious substring, and `pty_support`'s two header constants carry
/// the reason. An unconditional settle here returns before raw mode is on — the grid is not
/// blank, the pre-alt-screen preferences warning is in it, so `screen_retry` has nothing to
/// retry — and `"browser"` after the `Tab` is already true of the Dashboard's own footer.
/// Either one lets a `Tab` that the line discipline swallowed pass for a successful switch.
fn to_browser(home: &std::path::Path) -> Session {
    let mut session = Session::spawn_with_home(&fixture_path("loaded.json"), 90, 40, home);
    settle_until(&mut session, DASHBOARD_HEADER);
    send(&mut session, b"\t");
    let screen = settle_until(&mut session, BROWSER_HEADER);
    assert!(
        screen.iter().any(|r| r.contains(BROWSER_HEADER)),
        "expected to be on the Browser after Tab, got:\n{}",
        screen.join("\n")
    );
    session
}

#[test]
fn help_popup_opens_and_closes_on_any_key() {
    let home = scratch_home("help");
    let mut session = to_browser(&home);
    // "any key closes" is the popup's own footer — chrome that exists nowhere else, so it
    // separates the opened frame from the Browser underneath. "help" would not: the
    // Browser's footer advertises `? help`, so waiting for it is satisfied before `?` is
    // processed at all.
    send(&mut session, b"?");
    let opened = settle_until(&mut session, "any key closes");
    assert!(
        opened.iter().any(|r| r.contains("help")),
        "expected the help popup title after pressing ?, got:\n{}",
        opened.join("\n")
    );
    // Any key closes it, including one with its own normal-mode binding —
    // proving the popup consumed the keystroke rather than letting it fall
    // through to e.g. move_selection.
    send(&mut session, b"j");
    let closed = settle_until_gone(&mut session, "any key closes");
    assert!(
        !closed.iter().any(|r| r.contains("any key closes")),
        "expected the help popup to be gone after any keypress, got:\n{}",
        closed.join("\n")
    );
}
