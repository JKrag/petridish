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
//! notice without ever launching anything — no side effect on the machine running the tests.
//!
//! **One project, not the four-project `loaded.json` fixture.** An earlier version of this
//! file walked down with repeated `j` to reach a specific row, settling on an all-blank-grid
//! retry between each keystroke — exactly the settle-then-assert shape `CLAUDE.md` and
//! `s8_pty_handoff.rs` call out as this suite's root cause of flakiness: a frame from before
//! a `j` was processed satisfies "the row is visible" just as well as one from after, so a
//! swallowed keystroke lands `o` on the wrong project rather than failing loudly. A
//! single-project state file (built the same way `s14_pty_mini_actions.rs` builds its own)
//! removes that unbounded walk — there is exactly one row to reach. It does **not** remove
//! navigation entirely: the cursor still lands on the section header first
//! (`DashboardState::rebuild`'s first stop is always a header), so `spawn_alpha_02` still
//! sends one `j`, the same single deterministic step `s11_pty_focus.rs` takes for the same
//! reason — that step is load-bearing, not vestigial.

mod pty_support;
use pty_support::Session;
use std::io::Write;
use std::time::Duration;

/// A radar with exactly one project, derived from the real `loaded.json` fixture's
/// `alpha-02` entry (RUNNING/`active`, `github_url: null`) so it cannot drift out of step
/// with the schema `petri` actually parses, and so it lands pre-selected as the Dashboard's
/// only row.
fn state_file_with_one_no_remote_project(dir: &std::path::Path) -> std::path::PathBuf {
    let text = std::fs::read_to_string(pty_support::fixture_path("loaded.json"))
        .expect("the real fixture must be readable");
    let mut radar: serde_json::Value =
        serde_json::from_str(&text).expect("the real fixture must parse");
    let project = radar["projects"]
        .as_array()
        .expect("fixture must have a projects array")
        .iter()
        .find(|p| p["name"] == "alpha-02")
        .cloned()
        .expect("fixture must contain alpha-02");
    radar["projects"] = serde_json::json!([project]);

    let path = dir.join("radar.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&radar).expect("serialize"),
    )
    .expect("state file must be writable");
    path
}

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

/// Lands with the cursor on the RUNNING header — a fleet's first stop is always its
/// section header, per `DashboardState::rebuild` — so one `j` moves it onto the project row
/// itself, the same single deterministic step `s11_pty_focus.rs`'s tests take for the
/// identical reason (see e.g. `space_on_a_project_row_opens_the_focus_popup`).
fn spawn_alpha_02(home: &std::path::Path) -> Session {
    let state_path = state_file_with_one_no_remote_project(home);
    let mut session = Session::spawn_with_home(&state_path, 90, 40, home);
    settle_until(&mut session, "alpha-02");
    send(&mut session, b"j");
    let _ = session.screen_retry(
        90,
        40,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
    );
    session
}

#[test]
fn an_action_on_the_dashboard_reaches_the_selected_project() {
    let home = scratch_home("plain");
    let mut session = spawn_alpha_02(&home);

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
    let mut session = spawn_alpha_02(&home);

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
