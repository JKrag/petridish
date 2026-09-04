//! S4 acceptance gate, layer 3 (petri/SPEC.md §8, ADR-0003) — protected, authored
//! by the orchestrator, not the delegate. Drives the real compiled `petri` binary
//! in a PTY via the shared harness in `pty_support` (extracted here once S5
//! needed the identical harness — see that module's doc comment for the two real
//! PTY bugs found and fixed while building this file, and for why the harness
//! looks the way it does). No fixed-clock injection needed yet — S4 renders no
//! time-derived text.
//!
//! Was `#[ignore]`d while `petri::run`/`app::render` were `todo!()` stubs
//! (all three tests failed on that basis, confirmed before delegating S4);
//! stripped now that S4 landed and all three pass, including 4 consecutive
//! re-runs to rule out the exact flakiness ADR-0003 warns this layer is prone
//! to.

mod pty_support;
use pty_support::{Session, fixture_path};
use std::io::Write;
use std::time::Duration;

#[test]
fn missing_state_file_exits_one_with_message() {
    // Retry wrapper for a real, narrow OS-level PTY race: this is petri's
    // FASTEST exit path (one eprintln! then exit(1), no terminal setup at
    // all), and on macOS an already-buffered write can occasionally be lost
    // if the slave side closes fast enough after writing — observed at
    // roughly 1/15-1/30 runs even with the master pty kept alive and the
    // reader thread started before the child spawns (see pty_support's
    // module doc comment for the mitigations already in place). ADR-0003 is
    // explicit that a flaky layer-3 test is worse than none in an unattended
    // context, so rather than accept a nonzero flake rate, retry the whole
    // spawn+settle cycle up to 3 times and only fail if it's consistently
    // empty — which would mean a real regression, not this race.
    let missing = std::env::temp_dir().join(format!("petri_s4_pty_missing_{}.json", std::process::id()));
    let _ = std::fs::remove_file(&missing);

    let mut output = String::new();
    let mut status = None;
    for attempt in 1..=3 {
        let mut session = Session::spawn(&missing, 80, 24);
        output = session.settle(Duration::from_secs(5), Duration::from_millis(300));
        status = Some(session.wait_with_timeout(Duration::from_secs(5)));
        if !output.is_empty() {
            break;
        }
        eprintln!("attempt {attempt}/3: empty output (suspected PTY race), retrying");
    }
    let status = status.expect("at least one attempt must have run");

    assert_eq!(
        status.exit_code(),
        1,
        "missing state file must exit 1, got exit code with output: {output:?}"
    );
    assert!(
        output.contains("no state file at") && output.contains("swab scan"),
        "missing state file must print the shared swab list/path message after 3 attempts, got: {output:?}"
    );
}

#[test]
fn q_quits_cleanly_and_restores_the_terminal() {
    let mut session = Session::spawn(&fixture_path("minimal.json"), 80, 24);
    let first_frame = session.settle(Duration::from_secs(5), Duration::from_millis(300));
    assert!(
        first_frame.contains("petri"),
        "first frame must render before we send any keystroke, got: {first_frame:?}"
    );

    session.writer.write_all(b"q").expect("write 'q' must succeed");
    let status = session.wait_with_timeout(Duration::from_secs(5));

    assert_eq!(status.exit_code(), 0, "'q' must exit 0");
    // Terminal restoration: the accumulated output must contain the leave-
    // alternate-screen sequence somewhere — if the panic-hook / normal-exit
    // teardown regresses, this is the first thing that stops appearing.
    let tail = session.settle(Duration::from_millis(500), Duration::from_millis(200));
    let whole = format!("{first_frame}{tail}");
    assert!(
        whole.contains("\u{1b}[?1049l"),
        "exit must leave the alternate screen (\\x1b[?1049l) at some point in the output, got: {whole:?}"
    );
}

#[test]
fn survives_a_resize_to_a_degenerate_geometry() {
    // A freshly-forked pty can report 0x0 (petri/SPEC.md §4) — start tiny rather
    // than resizing after the fact, since portable-pty's own openpty already
    // exercises the same code path petri must not panic on.
    let mut session = Session::spawn(&fixture_path("minimal.json"), 1, 1);
    let output = session.settle(Duration::from_secs(5), Duration::from_millis(300));
    // Either it rendered *something* (a "resize terminal" message counts) or
    // it's still alive waiting — what it must NOT do is have already crashed.
    let alive = session.child.try_wait().ok().flatten().is_none();
    assert!(
        alive,
        "petri must not crash on a degenerate 1x1 geometry, output so far: {output:?}"
    );
    session.writer.write_all(b"q").ok();
    let _ = session.wait_with_timeout(Duration::from_secs(5));
}
