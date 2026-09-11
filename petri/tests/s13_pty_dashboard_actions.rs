//! Issue #64, layer 3: tool shortcuts (`o`/`e`/`g`/...) against the real binary on the
//! Dashboard, both with the focus popup closed and with it open.
//!
//! Before this fix, `Screen::Dashboard`'s key match had no `Char(c)` arm at all — every
//! registry action key was a silent no-op there, popup open or closed, and even a project
//! selected under the popup routed nowhere. `s8_pty_actions.rs` already gates the
//! equivalent behaviour on the Browser; this file is its Dashboard counterpart.
//!
//! `o` on a no-remote project is deliberately the probe throughout, for the same reason
//! `s8_pty_actions.rs` uses it: `open` is installed on every dev machine, so tool
//! availability is never the reason this stops short, and `Resolution::NoTarget` produces a
//! notice without ever launching anything — no side effect on the machine running the
//! tests. `alpha-02` is the fixture's first project with `github_url: null`.

mod pty_support;
use pty_support::{DASHBOARD_HEADER, Session, fixture_path};
use std::io::Write;
use std::time::Duration;

fn scratch_home(tag: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!(
        "petri_s13_pty_dash_{tag}_home_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("scratch home dir must be creatable");
    home
}

fn settle_until(session: &mut Session, needle: &'static str) -> Vec<String> {
    session.screen_until(
        90,
        40,
        Duration::from_secs(5),
        Duration::from_millis(300),
        6,
        |grid| grid.iter().any(|r| r.contains(needle)),
    )
}

fn send(session: &mut Session, bytes: &[u8]) {
    session.writer.write_all(bytes).expect("write must succeed");
    session.writer.flush().expect("flush must succeed");
}

/// Land on the Dashboard with `alpha-02` (no remote) selected. RUNNING's rows are sorted by
/// name in the fixture used elsewhere in this suite; rather than depend on that ordering
/// this walks down with `j` until the row is on screen, which is what a real user does too.
fn to_alpha_02(home: &std::path::Path) -> Session {
    let mut session = Session::spawn_with_home(&fixture_path("loaded.json"), 90, 40, home);
    settle_until(&mut session, DASHBOARD_HEADER);
    for _ in 0..40 {
        let screen = session.screen_retry(
            90,
            40,
            Duration::from_secs(2),
            Duration::from_millis(150),
            3,
        );
        if screen.iter().any(|r| r.contains("alpha-02")) {
            break;
        }
        send(&mut session, b"j");
    }
    session
}

#[test]
fn an_action_on_the_dashboard_reaches_the_selected_project() {
    let home = scratch_home("plain");
    let mut session = to_alpha_02(&home);

    send(&mut session, b"o");
    let screen = settle_until(&mut session, "no remote");
    let body = screen.join("\n");
    assert!(
        body.contains("no remote"),
        "an action key on the Dashboard must reach begin_action and its notice must be \
         drawn (issue #64), got:\n{body}"
    );
    assert!(
        body.contains("dashboard"),
        "the notice must be drawn on top of the Dashboard itself, not just computed and \
         discarded, got:\n{body}"
    );

    send(&mut session, b"q");
    session.wait_with_timeout(Duration::from_secs(10));
}

#[test]
fn an_action_still_fires_while_the_focus_popup_is_open() {
    // The popup has no key handling of its own — it renders whatever `selected` points at,
    // so a project row under an open popup must dispatch exactly like a closed one.
    let home = scratch_home("popup");
    let mut session = to_alpha_02(&home);

    send(&mut session, b" ");
    let opened = settle_until(&mut session, "Focus");
    assert!(
        opened.iter().any(|r| r.contains("Focus")),
        "precondition: the popup must be open, got:\n{}",
        opened.join("\n")
    );

    send(&mut session, b"o");
    let screen = settle_until(&mut session, "no remote");
    let body = screen.join("\n");
    assert!(
        body.contains("no remote"),
        "an action key must still dispatch to the selected project while the focus popup \
         is open (issue #64), got:\n{body}"
    );

    send(&mut session, b"q");
    session.wait_with_timeout(Duration::from_secs(10));
}
