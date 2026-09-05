//! `ACT-10` (petri/IDEAS.md §2), layer 3: the filter query against the real
//! binary.
//!
//! Layer 2 (`s8_filter_chip.rs`) proves `browser::render` draws the chip from
//! a hand-built `BrowserState`. Only this layer proves the event loop puts
//! the state into that shape — that `/` and the keys after it reach the
//! filter and set `filter_input`, and that `Enter` closes the input without
//! discarding the query. ACT-10 noted the missing display "costs the PTY
//! layer a natural assertion target"; this is that target.
//!
//! No launching, no side effects: only `/`, letters, `Enter` and `q`.

mod pty_support;
use pty_support::{Session, fixture_path};
use std::io::Write;
use std::time::Duration;

fn scratch_home(name: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("petri_act10_pty_{name}_{}", std::process::id()));
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
fn the_typed_query_appears_on_screen_and_survives_enter() {
    let home = scratch_home("query_visible");
    let mut session = to_browser(&home);

    send(&mut session, b"/");
    settle(&mut session);
    send(&mut session, b"beta");
    let typing = settle(&mut session).join("\n");
    assert!(
        typing.contains("/beta"),
        "the query typed into the `/` filter must be visible, got:\n{typing}"
    );

    // `Enter` closes the input but keeps the query (SPEC.md §3.1). The whole
    // point of ACT-10 is that this state is still legible.
    send(&mut session, b"\r");
    let kept = settle(&mut session).join("\n");
    assert!(
        kept.contains("/beta"),
        "the query must remain visible after Enter closes the input, got:\n{kept}"
    );

    // `Esc` in normal mode is a no-op, so re-open the filter and clear it
    // there — the chip must then disappear entirely.
    send(&mut session, b"/");
    settle(&mut session);
    send(&mut session, &[0x1b]);
    let cleared = settle(&mut session).join("\n");
    assert!(
        !cleared.contains("/beta"),
        "Esc must clear the query and take the chip with it, got:\n{cleared}"
    );

    send(&mut session, b"q");
    let status = session.wait_with_timeout(Duration::from_secs(5));
    assert_eq!(
        status.exit_code(),
        0,
        "`q` must still exit 0 after filtering"
    );
}

#[test]
fn backspace_deletes_the_last_character_and_refilters() {
    // A typo used to be unrecoverable: `Esc` and retype, or live with it.
    // This is the layer that can actually prove the fix, because Backspace
    // arrives as a real terminal byte (0x7f, DEL — what every terminal on
    // this machine sends for the Backspace key, NOT 0x08) and only the real
    // binary decodes it.
    let home = scratch_home("backspace");
    let mut session = to_browser(&home);

    send(&mut session, b"/");
    settle(&mut session);
    send(&mut session, b"bravoX");
    let typo = settle(&mut session).join("\n");
    assert!(
        typo.contains("/bravoX"),
        "setup: the typo must be on screen before we delete it, got:\n{typo}"
    );
    assert!(
        typo.contains("0 of "),
        "setup: \"bravoX\" must match nothing, or the delete proves nothing, got:\n{typo}"
    );

    send(&mut session, &[0x7f]);
    let fixed = settle(&mut session).join("\n");
    assert!(
        fixed.contains("/bravo") && !fixed.contains("/bravoX"),
        "Backspace must drop exactly the last character, got:\n{fixed}"
    );
    assert!(
        !fixed.contains("0 of "),
        "the list must re-filter on Backspace, not just redraw the query:\n{fixed}"
    );

    // Backspacing past the start is a no-op, not a panic or an underflow.
    for _ in 0..8 {
        send(&mut session, &[0x7f]);
    }
    let emptied = settle(&mut session).join("\n");
    assert!(
        emptied.contains("browser"),
        "petri must survive Backspace on an empty query, got:\n{emptied}"
    );

    send(&mut session, b"q");
    let status = session.wait_with_timeout(Duration::from_secs(5));
    assert_eq!(
        status.exit_code(),
        0,
        "`q` must still exit 0 after backspacing"
    );
}
