//! S5 acceptance gate, layer 3 (petri/SPEC.md §3.1/§8, ADR-0003) — protected,
//! authored by the orchestrator, not the delegate. Real keystrokes against the
//! compiled `petri` binary via the shared `pty_support` harness (see that
//! module's doc comment for the PTY bugs already found/fixed and why the
//! harness looks the way it does).
//!
//! Layer 3's actual job (ADR-0003: "the only thing covering whether q
//! actually gives you your shell back") is plumbing/lifecycle, not visual
//! content — that's layer 1 (`s5_selection.rs`, which exhaustively covers the
//! real selection-movement logic) and layer 2's job. The one exception here
//! is the initial-frame section-label check, which is the one thing S4's
//! already-working flat-list stub structurally cannot produce (it has no
//! section concept at all) and so is the one assertion that actually
//! discriminates "S5 implemented" from "still the stub" — every other
//! plausible PTY-level check (e.g. "does *something* redraw on a keypress")
//! turned out to trivially pass against S4 too, since its poll loop already
//! redraws unconditionally on any key event.
//!
//! Requires `lib.rs`'s `poll_loop` to route key events into a live
//! `BrowserState` and render via `browser::render` — currently it calls
//! `app::render` (S4's flat-list placeholder, no section labels), so the
//! section-label check FAILS on that basis, confirmed before delegating S5.

mod pty_support;
use pty_support::{Session, fixture_path};
use std::io::Write;
use std::time::Duration;

#[test]
fn initial_frame_shows_section_labels() {
    // Bounded retry for the same class of OS-level PTY race documented in
    // pty_support's module doc comment and s4_pty.rs's missing-state-file
    // test: observed here at roughly 1/3 runs (measured empirically — this
    // slice's real terminal setup, alternate screen + raw mode, appears to
    // widen the race window versus S4's simpler flat-list render). ADR-0003
    // is explicit that a flaky layer-3 test is worse than none in an
    // unattended context, so rather than accept a nonzero flake rate, retry
    // the whole spawn+settle cycle up to 3 times and only fail if it's
    // consistently empty — which would mean a real regression, not this race.
    let mut first_frame = String::new();
    for attempt in 1..=3 {
        let mut session = Session::spawn(&fixture_path("normal.json"), 80, 40);
        first_frame = session.settle(Duration::from_secs(5), Duration::from_millis(300));
        session.writer.write_all(b"q").ok();
        let _ = session.wait_with_timeout(Duration::from_secs(5));
        if !first_frame.is_empty() {
            break;
        }
        eprintln!("attempt {attempt}/3: empty output (suspected PTY race), retrying");
    }
    // normal.json populates every bucket (5 active / 4 in_flight / 4 stale /
    // 3 cold) — see s5_snapshot.rs's identical assertion for why this is the
    // one check that actually discriminates S5 from S4's stub.
    for label in ["RUNNING", "IN FLIGHT", "STALE", "COLD"] {
        assert!(
            first_frame.contains(label),
            "initial frame must show section label {label:?} after 3 attempts, got: {first_frame:?}"
        );
    }
}

#[test]
fn navigation_and_filter_keystrokes_do_not_crash_the_binary() {
    // Lifecycle/plumbing check, not a content check (see module doc comment):
    // j/k/arrows move selection, / opens the filter, Esc clears it — none of
    // this should crash or hang the real binary end-to-end.
    let mut session = Session::spawn(&fixture_path("normal.json"), 80, 24);
    let _ = session.settle(Duration::from_secs(5), Duration::from_millis(300));

    for keys in [&b"j"[..], b"j", b"k", b"/", b"ab", &[0x1b]] {
        session
            .writer
            .write_all(keys)
            .unwrap_or_else(|e| panic!("write {keys:?} must succeed: {e}"));
        let _ = session.settle(Duration::from_millis(500), Duration::from_millis(150));
        let alive = session.child.try_wait().ok().flatten().is_none();
        assert!(alive, "petri must still be alive after sending {keys:?}");
    }

    session
        .writer
        .write_all(b"q")
        .expect("write 'q' must succeed");
    let status = session.wait_with_timeout(Duration::from_secs(5));
    assert_eq!(
        status.exit_code(),
        0,
        "'q' must still exit 0 after a sequence of navigation/filter keystrokes"
    );
}

#[test]
fn q_still_quits_cleanly_with_browser_active() {
    // Regression guard: S5 must not break S4's basic "q quits" contract while
    // wiring BrowserState into the event loop.
    let mut session = Session::spawn(&fixture_path("normal.json"), 80, 24);
    let _ = session.settle(Duration::from_secs(5), Duration::from_millis(300));
    session
        .writer
        .write_all(b"q")
        .expect("write 'q' must succeed");
    let status = session.wait_with_timeout(Duration::from_secs(5));
    assert_eq!(
        status.exit_code(),
        0,
        "'q' must still exit 0 with the Browser wired in"
    );
}
