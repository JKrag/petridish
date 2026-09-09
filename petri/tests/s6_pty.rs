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
//!
//! All three tests use `spawn_and_settle_nonempty_with_home` (a scratch
//! `HOME`), not bare `spawn_and_settle_nonempty` — this file's assertions
//! assume Dashboard is the default landing screen, which S7 made dependent
//! on `~/.petridish/petri.toml`'s `last_screen`. A bare spawn inherits the
//! ambient `$HOME`, which silently broke this exact assumption once a real
//! developer machine's prefs file happened to say `last_screen = "browser"`
//! from unrelated manual testing — caught by these tests actually failing
//! against real environment state, not a fixture.

mod pty_support;
use pty_support::{Session, fixture_path, spawn_and_settle_nonempty_with_home};
use std::io::Write;
use std::time::Duration;

fn scratch_home(name: &str) -> std::path::PathBuf {
    let home =
        std::env::temp_dir().join(format!("petri_s6_pty_{name}_home_{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("scratch home dir must be creatable");
    home
}

#[test]
fn initial_frame_shows_the_dashboard_header_and_a_populated_section_label() {
    let home = scratch_home("initial_frame");
    let (mut session, first_frame) = spawn_and_settle_nonempty_with_home(
        &fixture_path("loaded.json"),
        80,
        40,
        &home,
        Duration::from_secs(5),
        Duration::from_millis(300),
        3,
    );
    session.writer.write_all(b"q").ok();
    let _ = session.wait_with_timeout(Duration::from_secs(5));

    assert!(
        first_frame.contains("petri"),
        "initial frame must contain \"petri\" after 3 attempts, got: {first_frame:?}"
    );
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
fn enter_on_a_row_switches_from_dashboard_to_browser() {
    let home = scratch_home("enter_switches");
    // Waiting for the FRAME, not for bytes. `spawn_and_settle_nonempty_with_home` retries
    // only while the output is empty, and petri's first write here is not a frame at all —
    // it is S7's "preferences file missing, using defaults" warning on stderr, emitted
    // before the alternate screen is even entered. That makes the output non-empty
    // immediately, the retry loop declares success, and the precondition sees a warning
    // line instead of a dashboard. Measured at 2 failures in 64 runs with eight of these
    // sessions in parallel, all of them exactly this.
    let mut session = Session::spawn_with_home(&fixture_path("loaded.json"), 80, 40, &home);
    let first_frame = session.screen_until(
        80,
        40,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
        |grid| grid.iter().any(|r| r.contains("petri · dashboard")),
    );
    assert!(
        first_frame.iter().any(|r| r.contains("petri · dashboard")),
        "precondition: petri must start on the Dashboard, or the assertions below would pass vacuously without exercising the Enter->Browser transition at all. Got:\n{}",
        first_frame.join("\n")
    );

    // Cursor starts on RUNNING's header (the first stop) — one `j` moves it
    // onto RUNNING's first project row (loaded.json's RUNNING is expanded by
    // default and non-empty), so this `Enter` targets a row, not a header.
    session
        .writer
        .write_all(b"j")
        .expect("write 'j' must succeed");
    let _ = session.settle(Duration::from_millis(500), Duration::from_millis(150));
    session
        .writer
        .write_all(b"\r")
        .expect("write Enter must succeed");
    // `screen_until` on the grid, not `settle` on the raw stream, and both halves of that
    // matter.
    //
    // The wait: a plain settle returns whatever has arrived when the quiet window elapses,
    // which after a keystroke can be the frame from *before* the key was processed.
    // Measured at 3 failures in 48 runs with eight of these PTY sessions running at once —
    // which is what `make check` does, and why this reproduced there and not under plain
    // CPU load.
    //
    // The grid: the NOTE below describes working around ratatui's diff-based redraw by
    // hunting for a marker in the raw byte stream, because "petri · browser" is never
    // written contiguously. Reconstructing the grid is the answer to that — it replays the
    // cursor-positioned partial writes into the cells they actually land in — and
    // `SPEC.md` §8 names raw substring matching as the root cause of the Python TUI's worst
    // CI flakiness.
    let after_enter = session.screen_until(
        80,
        40,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
        |grid| grid.iter().any(|r| r.contains("Projects")),
    );

    session.writer.write_all(b"q").ok();
    let _ = session.wait_with_timeout(Duration::from_secs(5));

    // The marker is one unique to the Browser's own vocabulary that the Dashboard never
    // emits: "Projects", the list pane's title in `browser.rs`. Asserting the *absence* of
    // the Dashboard's text would not work even on the grid — the header is one cell wide in
    // difference — and on the raw stream it could never work, since `settle` accumulates
    // across the whole session and the first frame's bytes are there forever.
    let body = after_enter.join("\n");
    assert!(
        after_enter.iter().any(|r| r.contains("petri")),
        "post-Enter frame must still contain \"petri\", got:\n{body}"
    );
    assert!(
        after_enter.iter().any(|r| r.contains("Projects")),
        "Enter on a Dashboard row must jump to the Browser (petri/SPEC.md §3.2/§5) — checking for the Browser-only \"Projects\" list-pane marker, got:\n{body}"
    );
}

#[test]
fn dashboard_keystrokes_do_not_crash_the_binary() {
    let home = scratch_home("keystrokes");
    let (mut session, _first_frame) = spawn_and_settle_nonempty_with_home(
        &fixture_path("loaded.json"),
        80,
        24,
        &home,
        Duration::from_secs(5),
        Duration::from_millis(300),
        3,
    );

    for keys in [&b"j"[..], b"j", b"k", b" ", b"j", b" ", &[0x1b]] {
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
        "'q' must still exit 0 after a sequence of Dashboard navigation/toggle keystrokes"
    );
}
