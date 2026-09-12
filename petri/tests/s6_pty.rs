//! S6 acceptance gate, layer 3 (petri/SPEC.md §3.2/§8, ADR-0003) — protected,
//! authored by the orchestrator, not the delegate. Real keystrokes against the
//! compiled `petri` binary via the shared `pty_support` harness.
//!
//! Layer 3's job here (as in S4/S5) is plumbing/lifecycle, not visual
//! content — that's layer 1 (`s6_dashboard.rs`) and layer 2
//! (`s6_snapshot.rs`)'s job. The initial-frame header/RUNNING-label check
//! that used to sit here was removed by the #48 audit: `s6_snapshot.rs`'s
//! `header_identifies_the_dashboard_screen_at_80x24` and
//! `running_label_rendered_for_loaded_json_which_has_agents_present` assert
//! the identical thing against the identical fixture, with no process and
//! no timing.
//!
//! `enter_on_a_row_switches_from_dashboard_to_browser` — the one content
//! check kept here through #48's audit, because at the time it was the only
//! way to prove `Enter` on a row switches screens end-to-end through the
//! real event loop (a `poll_loop` dispatch property, not a rendering one,
//! and `poll_loop` was hardcoded to `Terminal<CrosstermBackend<Stdout>>`
//! rather than generic over `Backend`) — has itself now moved, to
//! `s61_key_dispatch.rs`'s `enter_on_a_dashboard_row_switches_to_browser`.
//! Issue #61 made `poll_loop`'s dispatch logic (`handle_key`) generic over
//! `Backend` and public, which is exactly the precondition this file's old
//! doc comment named as blocking the move. What remains here —
//! `dashboard_keystrokes_do_not_crash_the_binary` — is a real-process
//! crash/hang guard `TestBackend` cannot stand in for.
//!
//! Requires `lib.rs`'s `poll_loop` to default to the Dashboard screen (not
//! the Browser — petri/SPEC.md §3.2 frames it as "the ambient monitor" and
//! the primary landing screen; `Tab` switching, S7, is not wired yet, so S6
//! must make Dashboard reachable as the default or `Enter`→Browser could
//! never be exercised at all) and route `Space`/`Enter`/`j`/`k` into a live
//! `DashboardState`. Confirmed failing against the current stub (`lib.rs`
//! still only knows the Browser) before delegating S6.
//!
//! Spawns with a scratch `HOME` rather than the ambient one — this file's
//! assertion assumes Dashboard is the default landing screen, which S7 made dependent
//! on `~/.petridish/petri.toml`'s `last_screen`. A bare spawn inherits the
//! ambient `$HOME`, which silently broke this exact assumption once a real
//! developer machine's prefs file happened to say `last_screen = "browser"`
//! from unrelated manual testing — caught by this test actually failing
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
