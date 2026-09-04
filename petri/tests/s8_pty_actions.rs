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
    settle(&mut session);
    send(&mut session, b"g");
    let screen = settle(&mut session);

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
    send(&mut session, b"/alpha-02");
    settle(&mut session);
    send(&mut session, b"\r");
    settle(&mut session);

    send(&mut session, b"o");
    let screen = settle(&mut session);
    let body = screen.join("\n");
    assert!(
        body.contains("no remote"),
        "expected a notice explaining the project has no remote, got:\n{body}"
    );

    send(&mut session, b"q");
    session.wait_with_timeout(Duration::from_secs(10));
}
