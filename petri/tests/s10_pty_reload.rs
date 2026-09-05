//! Regression gate: a state-file reload must not undo the user's own layout
//! choices (petri/SPEC.md §3.2, §4.3).
//!
//! Reported from real use: "I collapse IN FLIGHT to see more of the event log,
//! but occasionally I see it automatically un-collapsing, through no
//! interaction by me." The cause was in `lib.rs`'s poll loop, which rebuilt the
//! Dashboard with `DashboardState::new` on every mtime change — and that
//! constructor hardcodes the spec's *default* collapse state. `swab` rewrites
//! the state file every few seconds on an active machine, so any section the
//! user had collapsed reopened itself on the next tick.
//!
//! This has to be a PTY test, for the same reason `s8_pty_prefs.rs` does: the
//! pure-state tests in `s6_dashboard.rs` pin `DashboardState::refresh`'s
//! contract, and they all passed while the bug was live, because the defect was
//! at the *call site* — the poll loop simply never called the preserving path.
//! Only the real binary, reloading a real file, exercises that seam.
//!
//! Determinism, per petri/SPEC.md §8 layer 3: the collapse state is seeded
//! through `petri.toml` rather than by navigating and pressing `Space`, so the
//! test presses no keys and depends on no cursor arithmetic; assertions are on
//! a settled full-screen snapshot, never the raw byte stream; and the assertion
//! is a section-header line from a fixed fixture, never anything wall-clock
//! derived.

mod pty_support;
use pty_support::{Session, fixture_path};
use std::time::Duration;

/// Whether any IN FLIGHT project's row is on screen *in the fleet area*.
///
/// A collapsed section renders its header and count (in the tab strip, e.g.
/// `IN FLIGHT 4  ·  STALE 4  ·  COLD 3`) but none of its rows, so the presence of a
/// member's row is what actually distinguishes collapsed from expanded — the
/// section label itself is on screen either way.
///
/// The search stops at the ` ACTIVITY` label, and that bound is load-bearing rather
/// than tidiness: the feed lists recent events *by project name*, including projects
/// whose section is collapsed, so an unbounded search matches those feed rows and
/// reports "expanded" for a correctly collapsed section. Two earlier drafts of this
/// test were vacuous — one compared the line containing "IN FLIGHT", which at an
/// overflowing geometry is the `… not shown:` marker and reads identically either
/// way; the next matched feed rows. Both passed with the bug deliberately
/// reinstated, which is the only reason they were caught.
fn in_flight_rows_visible(screen: &[String]) -> bool {
    const IN_FLIGHT_MEMBERS: [&str; 4] = ["ember-core", "forest-net", "glacier-db", "horizon-gui"];
    let fleet_end = screen
        .iter()
        .position(|l| l.trim() == "ACTIVITY")
        .unwrap_or(screen.len());
    screen[..fleet_end]
        .iter()
        .any(|l| IN_FLIGHT_MEMBERS.iter().any(|name| l.contains(name)))
}

#[test]
fn a_state_file_reload_does_not_reopen_a_collapsed_section() {
    let tag = format!("petri_s10_reload_{}", std::process::id());
    let home = std::env::temp_dir().join(&tag);
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".petridish")).expect("scratch home must be creatable");

    // IN FLIGHT collapsed alongside the two that ship collapsed, so the user's
    // choice is the ONLY thing distinguishing this from the defaults. If a
    // reload resets to `[false, false, true, true]`, index 1 flips back.
    std::fs::write(
        home.join(".petridish").join("petri.toml"),
        b"last_screen = \"dashboard\"\ncollapsed = [false, true, true, true]\n",
    )
    .expect("seed prefs must be writable");

    // A writable copy of the fixture — this test rewrites it mid-session.
    let state_path = std::env::temp_dir().join(format!("{tag}_projects.json"));
    let body = std::fs::read_to_string(fixture_path("normal.json")).expect("fixture readable");
    assert!(
        body.contains("\"scan_duration_ms\": 312"),
        "fixture's scan_duration_ms must be the value this test rewrites"
    );
    std::fs::write(&state_path, &body).expect("state copy must be writable");

    let mut session = Session::spawn_with_home(&state_path, 100, 50, &home);
    let before = session.screen_retry(
        100,
        50,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
    );
    assert!(
        !in_flight_rows_visible(&before),
        "the seeded prefs must start with IN FLIGHT collapsed, so its rows are hidden: {before:#?}"
    );
    assert!(
        before.iter().any(|l| l.contains("scan 0.3s")),
        "header must show the fixture's own scan duration before the reload: {before:#?}"
    );

    // Trigger exactly what `swab scan` triggers: a rewrite with a newer mtime.
    // The ONLY content change is the scan duration, which the header renders —
    // that is this test's proof that a reload actually happened. Without it the
    // test is vacuous: a rewrite that changes nothing visible is indistinguishable
    // from no reload at all, and an earlier draft of this test passed happily with
    // the bug reinstated for exactly that reason.
    std::fs::write(
        &state_path,
        body.replace("\"scan_duration_ms\": 312", "\"scan_duration_ms\": 9900"),
    )
    .expect("state rewrite must succeed");

    // The poll interval is 2-5s (SPEC §4.3), so the reload cannot have happened
    // yet; wait past it before snapshotting. `settle`/`screen_retry` return as
    // soon as the stream goes quiet, so they do NOT wait this out on their own.
    std::thread::sleep(Duration::from_secs(7));

    let after = session.screen_retry(
        100,
        50,
        Duration::from_secs(12),
        Duration::from_millis(400),
        6,
    );

    // Proof the reload landed. If this fails the test is inconclusive about the
    // collapse assertion below, so it is asserted first and separately.
    assert!(
        after.iter().any(|l| l.contains("scan 9.9s")),
        "the reload never landed, so this test proves nothing about collapse: {after:#?}"
    );

    assert!(
        !in_flight_rows_visible(&after),
        "a reload reopened the collapsed IN FLIGHT section — its rows are back on \
         screen, i.e. the user's layout changed with no input from them: {after:#?}"
    );

    drop(session);
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_file(&state_path);
}
