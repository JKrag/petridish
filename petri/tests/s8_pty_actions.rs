//! S8 acceptance gate, layer 3: action keys against the real binary
//! (petri/IDEAS.md `ACT-2`, `ACT-9`).
//!
//! Two behaviours that only the real binary can demonstrate, and one of them
//! is a trap worth stating plainly: `g`, `e` and `o` are ordinary printable
//! characters, so a naive binding fires them while the user is typing into the
//! `/` type-ahead filter. Typing `g` to filter for `graphql-api` would launch
//! a git browser instead. `lib.rs` keeps the two key branches structurally
//! separate so it cannot happen — this gates that it stays that way.
//!
//! Neither test launches anything real. The first never reaches a launch at
//! all, and the second targets a fixture project with no remote, so it stops
//! at `Resolution::NoTarget`. Pressing `o` on a project that *does* have a
//! `github_url` would open a browser window on the machine running the tests,
//! which is exactly the kind of side effect a test suite must not have.

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

/// Settle until the grid shows `needle`. Every assertion after a keystroke must go through
/// this. An unconditional settle cannot: `screen_retry` retries only an all-blank grid, so
/// the frame it returns can be a well-formed one from before the key was processed — which is
/// why the unconditional helper this file used to have is gone. This file measured 15
/// failures in 24 runs at eight-way concurrency before the conversion — see
/// `petri/scripts/flake-hunt.sh`.
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

/// Settle until `needle` is gone — the counterpart for asserting something disappeared.
fn settle_until_gone(session: &mut Session, needle: &'static str) -> Vec<String> {
    session.screen_until(
        90,
        40,
        Duration::from_secs(5),
        Duration::from_millis(300),
        6,
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
fn action_keys_do_not_fire_while_the_filter_has_focus() {
    // `g` is bound to git history in normal mode. Typed into the `/` filter it
    // must be a filter character and nothing else. None of the fixture's
    // project names (alpha-01..) contain a `g`, so a `g` that reached the
    // filter empties the list — and a `g` that was stolen by the action
    // dispatch would leave the query empty and the list fully populated.
    // That difference is the assertion.
    let home = scratch_home("filter_swallows_action_keys");
    let mut session = to_browser(&home);

    send(&mut session, b"/");
    settle_until(&mut session, "/");
    // The assertion below is an ABSENCE ("alpha-01" gone, because `g` filtered it out), and
    // an absence is satisfied by a frame that has not repainted yet just as well as by the
    // real thing. So the wait is for the list to have actually emptied — the same fact,
    // stated positively.
    send(&mut session, b"g");
    let screen = settle_until_gone(&mut session, "alpha-01");

    let body = screen.join("\n");
    assert!(
        !body.contains("alpha-01"),
        "the `g` never reached the filter — the list is still unfiltered, so the \
         action dispatch stole the keystroke:\n{body}"
    );
    assert!(
        body.contains("browser"),
        "petri must still be on screen — a launched git browser would have \
         replaced it:\n{body}"
    );

    send(&mut session, b"q");
    session.wait_with_timeout(Duration::from_secs(10));
}

#[test]
fn an_action_on_a_project_with_no_remote_reports_it() {
    // `ACT-9`'s per-project availability axis, end to end. `open` is installed
    // on this machine, so tool availability is satisfied and the only reason
    // to refuse is the project itself — and the message says so in terms of
    // the project, which is the half the user can see on the row.
    let home = scratch_home("no_remote_notice");
    let mut session = to_browser(&home);

    // alpha-02 is the fixture's first project with `github_url: null`.
    // Wait for the filter to apply before Enter: selecting from a list that has not
    // refiltered picks a different project, and the whole test is about which one.
    send(&mut session, b"/alpha-02");
    settle_until(&mut session, "/alpha-02");
    send(&mut session, b"\r");
    settle_until(&mut session, "alpha-02");

    send(&mut session, b"o");
    let screen = settle_until(&mut session, "no remote");
    let body = screen.join("\n");
    assert!(
        body.contains("no remote"),
        "expected a notice explaining the project has no remote, got:\n{body}"
    );

    send(&mut session, b"q");
    session.wait_with_timeout(Duration::from_secs(10));
}
