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
    let missing =
        std::env::temp_dir().join(format!("petri_s4_pty_missing_{}.json", std::process::id()));
    let _ = std::fs::remove_file(&missing);

    // Wait for EXIT, then drain. This invocation prints one message and terminates, so
    // there is no frame to wait for and no quiet window to guess at — and draining after
    // the child is gone collects everything it wrote, because the reader thread runs to
    // EOF and `settle` stops on the resulting disconnect. That removes the empty-output
    // race this loop was retrying around, rather than retrying it.
    let mut session = Session::spawn(&missing, 80, 24);
    let status = session.wait_with_timeout(Duration::from_secs(20));
    let output = session.settle(Duration::from_secs(5), Duration::from_millis(300));

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
    // Raw, not a grid: the assertion further down is about a control SEQUENCE
    // (`\x1b[?1049l`), which a reconstructed grid deliberately throws away. But the wait
    // still has to state its condition rather than trust a quiet window.
    //
    // The condition is the ALTERNATE-SCREEN ENTRY, not any text. `settle_until_raw`'s own
    // doc comment forbids content assertions over the raw stream, and `"petri"` was one —
    // a bad one, at that: `prefs::load` warns `petri S7: preferences file ... missing` and
    // lib.rs's "Step 1.5" emits it deliberately BEFORE `enable_raw_mode`, so that predicate
    // was satisfiable by a stderr line printed while the terminal was still in canonical
    // mode. A `q` written there is buffered by the line discipline and discarded when raw
    // mode comes on — the swallowed-keystroke bug, which surfaces seconds later as "child
    // did not exit". Raw mode is enabled immediately before `EnterAlternateScreen`, so the
    // entry is the exact, non-textual proof that the `q` below can be delivered.
    let mut first_frame = String::new();
    let ready = session.settle_until_raw(
        Duration::from_secs(5),
        Duration::from_millis(300),
        6,
        |stream| {
            first_frame = stream.to_string();
            Session::alt_screen_entries(stream) >= 1
        },
    );
    assert!(
        ready,
        "petri never entered the alternate screen, so raw mode was never on and the 'q' \
         below could not be delivered, got: {first_frame:?}"
    );

    // The entry proves the keystroke is deliverable; it does not prove anything was drawn,
    // and something has to. This is the only test in this file that runs at a normal
    // geometry — `missing_state_file_exits_one_with_message` never reaches a terminal and
    // `survives_a_resize_to_a_degenerate_geometry` is 1x1 by construction — so if it does
    // not assert that petri paints, nothing here does. The old `"petri"` predicate did
    // assert it on a machine whose ambient `$HOME` had a `petri.toml` (no warning to match
    // instead); replacing it with the entry alone would have quietly dropped that.
    //
    // On the GRID, not the raw stream: `settle_until_raw`'s doc comment reserves that for
    // mode transitions, and SPEC.md §8 is why. The needle is `"petri \u{b7} "` rather than
    // either full badge because this test inherits the ambient `$HOME`, so which screen it
    // lands on depends on a persisted `last_screen` — the same contamination `s5_pty.rs`
    // documents. Both badges share the prefix and the warning does not.
    let first_grid = session.screen_until(
        80,
        24,
        Duration::from_secs(5),
        Duration::from_millis(300),
        6,
        |grid| grid.iter().any(|r| r.contains("petri \u{b7} ")),
    );
    assert!(
        first_grid.iter().any(|r| r.contains("petri \u{b7} ")),
        "petri must paint a frame before we send any keystroke, got:\n{}",
        first_grid.join("\n")
    );

    session
        .writer
        .write_all(b"q")
        .expect("write 'q' must succeed");
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
    // Same readiness condition as the quit test above, and for the same reason: the
    // unconditional `settle` that used to be here returned as soon as the stream went
    // quiet, which can be before petri has taken the terminal at all — and at 1x1 there is
    // barely any output to keep it un-quiet. A `q` written then is buffered by the line
    // discipline in canonical mode and discarded when raw mode comes on.
    //
    // **What is measured and what is not, because the difference matters here.** This test
    // did fail 8 times in 24 during one full `make flake-hunt`, with the "child did not
    // exit within 5s" signature that a swallowed `q` produces. That the swallowed `q` was
    // the cause is NOT established: the pre-fix binary was then clean 24/24 at eight-way,
    // 24/24 at sixteen-way and 32/32 at thirty-two-way concurrency in isolation, so the
    // failure has not been reproduced outside that one full-hunt run and plain machine load
    // against the 5s budget below remains an equally good explanation. This change is made
    // because an unconditional settle before a keystroke is the exact pattern this PR
    // exists to remove and it is strictly stronger, not because it is a proven fix. If the
    // budget below trips again, that is the other hypothesis, and it is still live.
    let mut output = String::new();
    let entered = session.settle_until_raw(
        Duration::from_secs(5),
        Duration::from_millis(300),
        6,
        |stream| {
            output = stream.to_string();
            Session::alt_screen_entries(stream) >= 1
        },
    );
    // Either it rendered *something* (a "resize terminal" message counts) or
    // it's still alive waiting — what it must NOT do is have already crashed.
    let alive = session.child.try_wait().ok().flatten().is_none();
    assert!(
        alive,
        "petri must not crash on a degenerate 1x1 geometry, output so far: {output:?}"
    );
    assert!(
        entered,
        "petri must still take the terminal at 1x1 — it is alive, so it is stuck before \
         raw mode rather than crashed, output so far: {output:?}"
    );
    session.writer.write_all(b"q").ok();
    // Deliberately left at five seconds. This budget is what tripped when the `q` above was
    // being swallowed, and raising it was the tempting fix — but the swallowed keystroke is
    // fixed at its cause now, so a tight budget here buys a real signal rather than masking
    // one. If this starts failing again, that is information, not noise to be absorbed.
    let _ = session.wait_with_timeout(Duration::from_secs(5));
}
