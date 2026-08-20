//! S6 acceptance gate, layer 3 (petri/SPEC.md §3.2/§8, ADR-0003) — protected,
//! authored by the orchestrator, not the delegate. Real keystrokes against the
//! compiled `petri` binary via the shared `pty_support` harness.
//!
//! Layer 3's job here (as in S4/S5) is plumbing/lifecycle, not visual
//! content — that's layer 1 (`s6_dashboard.rs`) and layer 2
//! (`s6_snapshot.rs`)'s job. The one content check kept here is the
//! Dashboard-vs-Browser header discriminator, because it's the one thing
//! that proves `Enter` on a row actually switches screens end-to-end through
//! the real event loop, not just in `DashboardState`'s own unit tests.
//!
//! Requires `lib.rs`'s `poll_loop` to default to the Dashboard screen (not
//! the Browser — petri/SPEC.md §3.2 frames it as "the ambient monitor" and
//! the primary landing screen; `Tab` switching, S7, is not wired yet, so S6
//! must make Dashboard reachable as the default or `Enter`→Browser could
//! never be exercised at all) and route `Space`/`Enter`/`j`/`k` into a live
//! `DashboardState`. Confirmed failing against the current stub (`lib.rs`
//! still only knows the Browser) before delegating S6.

mod pty_support;
use pty_support::{fixture_path, spawn_and_settle_nonempty};
use std::io::Write;
use std::time::Duration;

#[test]
#[ignore = "S6 gate: Dashboard screen not wired into poll_loop yet; run explicitly with --ignored"]
fn initial_frame_shows_the_dashboard_header_and_a_populated_section_label() {
    let (mut session, first_frame) =
        spawn_and_settle_nonempty(&fixture_path("loaded.json"), 80, 40, Duration::from_secs(5), Duration::from_millis(300), 3);
    session.writer.write_all(b"q").ok();
    let _ = session.wait_with_timeout(Duration::from_secs(5));

    assert!(first_frame.contains("petri"), "initial frame must contain \"petri\" after 3 attempts, got: {first_frame:?}");
    assert!(
        first_frame.contains("petri · dashboard"),
        "petri must land on the Dashboard by default (Tab-switching isn't wired until S7). Checking the literal \"petri · dashboard\" header text, not a loose \"dashboard\" substring — the Browser's own footer already advertises \"Tab Dashboard\", which would make a loose check pass against the Browser too. Got: {first_frame:?}"
    );
    assert!(
        first_frame.contains("RUNNING") || first_frame.contains("RECENT"),
        "loaded.json populates RUNNING (25 active projects), so its header (RUNNING, or RECENT if degraded) must appear, got: {first_frame:?}"
    );
}

#[test]
#[ignore = "S6 gate: Dashboard screen not wired into poll_loop yet; run explicitly with --ignored"]
fn enter_on_a_row_switches_from_dashboard_to_browser() {
    let (mut session, first_frame) =
        spawn_and_settle_nonempty(&fixture_path("loaded.json"), 80, 40, Duration::from_secs(5), Duration::from_millis(300), 3);
    assert!(
        first_frame.contains("petri · dashboard"),
        "precondition: petri must start on the Dashboard, or the assertions below would pass vacuously without exercising the Enter->Browser transition at all. Got: {first_frame:?}"
    );

    // Cursor starts on RUNNING's header (the first stop) — one `j` moves it
    // onto RUNNING's first project row (loaded.json's RUNNING is expanded by
    // default and non-empty), so this `Enter` targets a row, not a header.
    session.writer.write_all(b"j").expect("write 'j' must succeed");
    let _ = session.settle(Duration::from_millis(500), Duration::from_millis(150));
    session.writer.write_all(b"\r").expect("write Enter must succeed");
    let after_enter = session.settle(Duration::from_secs(2), Duration::from_millis(300));

    session.writer.write_all(b"q").ok();
    let _ = session.wait_with_timeout(Duration::from_secs(5));

    assert!(after_enter.contains("petri"), "post-Enter frame must still contain \"petri\", got: {after_enter:?}");
    assert!(
        after_enter.contains("petri · browser"),
        "Enter on a Dashboard row must jump to the Browser (petri/SPEC.md §3.2/§5) — checking the literal \"petri · browser\" header text, got: {after_enter:?}"
    );
    assert!(
        !after_enter.contains("petri · dashboard"),
        "after switching to the Browser, the literal Dashboard header must no longer be on screen, got: {after_enter:?}"
    );
}

#[test]
#[ignore = "S6 gate: Dashboard screen not wired into poll_loop yet; run explicitly with --ignored"]
fn dashboard_keystrokes_do_not_crash_the_binary() {
    let (mut session, _first_frame) =
        spawn_and_settle_nonempty(&fixture_path("loaded.json"), 80, 24, Duration::from_secs(5), Duration::from_millis(300), 3);

    for keys in [&b"j"[..], b"j", b"k", b" ", b"j", b" ", &[0x1b]] {
        session.writer.write_all(keys).unwrap_or_else(|e| panic!("write {keys:?} must succeed: {e}"));
        let _ = session.settle(Duration::from_millis(500), Duration::from_millis(150));
        let alive = session.child.try_wait().ok().flatten().is_none();
        assert!(alive, "petri must still be alive after sending {keys:?}");
    }

    session.writer.write_all(b"q").expect("write 'q' must succeed");
    let status = session.wait_with_timeout(Duration::from_secs(5));
    assert_eq!(status.exit_code(), 0, "'q' must still exit 0 after a sequence of Dashboard navigation/toggle keystrokes");
}
