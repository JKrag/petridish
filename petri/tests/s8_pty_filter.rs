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

/// The filter input's block cursor (`browser.rs`'s `filter_input` branch). It is on screen
/// exactly while the input has focus, which makes it the one marker that distinguishes an
/// open filter from a closed one carrying the same query — see `enter_closes` below.
const INPUT_CURSOR: &str = "\u{2588}";

/// Settle until the grid shows `needle`. Every post-keystroke assertion goes through this
/// or `settle_until_gone`, never bare `settle`: `screen_retry` retries only an all-blank
/// grid, so what it returns after a keystroke may be a well-formed pre-keystroke frame.
/// This file measured 10 failures in 24 runs at eight-way concurrency before the
/// conversion — see `petri/scripts/flake-hunt.sh`.
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

/// Settle until `needle` is gone. Needed more often here than anywhere else: half of this
/// file's assertions are about something disappearing, and an absence is satisfied by a
/// frame that simply has not repainted.
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

fn to_browser(home: &std::path::Path) -> Session {
    let mut session = Session::spawn_with_home(&fixture_path("loaded.json"), 90, 40, home);
    settle(&mut session);
    send(&mut session, b"\t");
    let screen = settle_until(&mut session, "browser");
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
    settle_until(&mut session, INPUT_CURSOR);
    send(&mut session, b"beta");
    let typing = settle_until(&mut session, "/beta").join("\n");
    assert!(
        typing.contains("/beta"),
        "the query typed into the `/` filter must be visible, got:\n{typing}"
    );

    // `Enter` closes the input but keeps the query (SPEC.md §3.1). The whole
    // point of ACT-10 is that this state is still legible.
    // Waiting for "/beta" would prove nothing — it is already on screen, so the wait would
    // be satisfied by the pre-Enter frame and the assertion would pass without Enter having
    // been processed at all. The block cursor is what Enter actually removes.
    send(&mut session, b"\r");
    let kept = settle_until_gone(&mut session, INPUT_CURSOR).join("\n");
    assert!(
        kept.contains("/beta"),
        "the query must remain visible after Enter closes the input, got:\n{kept}"
    );

    // `Esc` in normal mode is a no-op, so re-open the filter and clear it
    // there — the chip must then disappear entirely.
    send(&mut session, b"/");
    settle_until(&mut session, INPUT_CURSOR);
    send(&mut session, &[0x1b]);
    let cleared = settle_until_gone(&mut session, "/beta").join("\n");
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
    settle_until(&mut session, INPUT_CURSOR);
    send(&mut session, b"bravoX");
    let typo = settle_until(&mut session, "/bravoX").join("\n");
    assert!(
        typo.contains("/bravoX"),
        "setup: the typo must be on screen before we delete it, got:\n{typo}"
    );
    assert!(
        typo.contains("0 of "),
        "setup: \"bravoX\" must match nothing, or the delete proves nothing, got:\n{typo}"
    );

    // Gone, not present: "/bravo" is a prefix of "/bravoX", so waiting for it would be
    // satisfied before the Backspace was ever processed.
    send(&mut session, &[0x7f]);
    let fixed = settle_until_gone(&mut session, "/bravoX").join("\n");
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
    let emptied = settle_until_gone(&mut session, "/b").join("\n");
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
