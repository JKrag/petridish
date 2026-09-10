//! S6 acceptance gate, layer 3 (petri/SPEC.md §3.2/§8, ADR-0003) — protected,
//! authored by the orchestrator, not the delegate. Real keystrokes against the
//! compiled `petri` binary via the shared `pty_support` harness.
//!
//! Layer 3's job here (as in S4/S5) is plumbing/lifecycle, not visual
//! content — that's layer 1 (`s6_dashboard.rs`) and layer 2
//! (`s6_snapshot.rs`)'s job. `enter_on_a_row_switches_from_dashboard_to_browser`
//! is the one content check kept here, because it's the one thing that
//! proves `Enter` on a row actually switches screens end-to-end through the
//! real event loop, not just in `DashboardState`'s own unit tests — that is
//! a `poll_loop` dispatch property, not a rendering one, and `poll_loop` is
//! hardcoded to `Terminal<CrosstermBackend<Stdout>>` rather than generic
//! over `Backend`, so there is nowhere else to put it today (issue #48's
//! audit; making `poll_loop`/`mini_poll_loop` generic the way `exec::run`
//! already is would be the precondition for moving it, and is out of scope
//! here). The initial-frame header/RUNNING-label check that used to sit
//! beside it was removed by that same audit: `s6_snapshot.rs`'s
//! `header_identifies_the_dashboard_screen_at_80x24` and
//! `running_label_rendered_for_loaded_json_which_has_agents_present` assert
//! the identical thing against the identical fixture, with no process and
//! no timing.
//!
//! Requires `lib.rs`'s `poll_loop` to default to the Dashboard screen (not
//! the Browser — petri/SPEC.md §3.2 frames it as "the ambient monitor" and
//! the primary landing screen; `Tab` switching, S7, is not wired yet, so S6
//! must make Dashboard reachable as the default or `Enter`→Browser could
//! never be exercised at all) and route `Space`/`Enter`/`j`/`k` into a live
//! `DashboardState`. Confirmed failing against the current stub (`lib.rs`
//! still only knows the Browser) before delegating S6.
//!
//! All three tests spawn with a scratch `HOME` rather than the ambient one — this file's
//! assertions assume Dashboard is the default landing screen, which S7 made dependent
//! on `~/.petridish/petri.toml`'s `last_screen`. A bare spawn inherits the
//! ambient `$HOME`, which silently broke this exact assumption once a real
//! developer machine's prefs file happened to say `last_screen = "browser"`
//! from unrelated manual testing — caught by these tests actually failing
//! against real environment state, not a fixture.

mod pty_support;
use pty_support::{Session, fixture_path};
use std::io::Write;
use std::time::Duration;

fn scratch_home(name: &str) -> std::path::PathBuf {
    let home =
        std::env::temp_dir().join(format!("petri_s6_pty_{name}_home_{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("scratch home dir must be creatable");
    home
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
    // Wait for a painted frame before sending anything. `spawn_and_settle_nonempty_with_home`
    // returns as soon as the output is non-empty, which can be before the binary has entered
    // raw mode — and a key written then is buffered by the line discipline in canonical mode
    // and discarded when raw mode is enabled. petri never sees it. That is the same bug
    // `s5_pty`'s quit test had, and it surfaces the same way: not as a wrong frame, but as
    // "child did not exit", seconds later. Measured at 4 failures in 24 runs at eight-way
    // concurrency.
    let mut session = Session::spawn_with_home(&fixture_path("loaded.json"), 80, 24, &home);
    session.screen_until(
        80,
        24,
        Duration::from_secs(5),
        Duration::from_millis(300),
        10,
        |grid| grid.iter().any(|r| r.contains("petri · dashboard")),
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
    // 20s, not 5: this is a hang detector, and a budget tight enough to trip on a busy
    // machine teaches people to re-run rather than to look.
    let status = session.wait_with_timeout(Duration::from_secs(20));
    assert_eq!(
        status.exit_code(),
        0,
        "'q' must still exit 0 after a sequence of Dashboard navigation/toggle keystrokes"
    );
}
