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
    // Waits for the FRAME, on the grid, rather than spawning up to three times and hoping
    // one of them has painted by the time the quiet window elapses. The old shape retried
    // only when the output was completely empty — but petri's first bytes need not be a
    // frame at all (a prefs warning on stderr is enough to make it non-empty), and a
    // partially painted frame is not empty either. Measured at 1 failure in 24 runs at
    // eight-way concurrency; see `petri/scripts/flake-hunt.sh`.
    //
    // Asserting on the reconstructed grid rather than the raw stream is the other half:
    // `SPEC.md` §8 names raw substring matching as the root cause of the Python TUI's worst
    // CI flakiness, and a diffed redraw need not emit a label contiguously.
    //
    // normal.json populates every bucket (5 active / 4 in_flight / 4 stale / 3 cold) — see
    // s5_snapshot.rs's identical assertion for why this is the one check that actually
    // discriminates S5 from S4's stub. COLD is last on screen, so waiting for it means all
    // four have been painted.
    let mut session = Session::spawn(&fixture_path("normal.json"), 80, 40);
    let first_frame = settle_grid_until(&mut session, 80, 40, |grid| {
        grid.iter().any(|r| r.contains("COLD"))
    });
    session.writer.write_all(b"q").ok();
    let _ = session.wait_with_timeout(Duration::from_secs(5));

    let body = first_frame.join("\n");
    for label in ["RUNNING", "IN FLIGHT", "STALE", "COLD"] {
        assert!(
            first_frame.iter().any(|r| r.contains(label)),
            "initial frame must show section label {label:?}, got:\n{body}"
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

fn settle_grid(session: &mut Session, cols: u16, rows: u16) -> Vec<String> {
    session.screen_retry(
        cols,
        rows,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
    )
}

/// Like `settle_grid`, but for a check that must observe the RESULT of a
/// keystroke just sent, not just the eventual first non-blank frame.
/// `screen_retry` (behind `settle_grid`) only retries on a blank grid; a
/// still-painted PREVIOUS frame (the redraw for this keystroke hasn't
/// happened yet) is not blank and would be returned as-is, failing the
/// assertion below even though the binary is about to do the right thing —
/// confirmed as the actual CI failure mode (see `screen_until`'s doc
/// comment in pty_support). Retries until `predicate` holds or the attempt
/// budget is spent, so a genuine regression still fails loudly.
fn settle_grid_until(
    session: &mut Session,
    cols: u16,
    rows: u16,
    predicate: impl FnMut(&[String]) -> bool,
) -> Vec<String> {
    session.screen_until(
        cols,
        rows,
        Duration::from_secs(5),
        Duration::from_millis(300),
        10,
        predicate,
    )
}

#[test]
fn space_toggles_the_detail_popup_at_a_hidden_geometry() {
    // Issue #35: at 60x10 (below DETAIL_PANE_THRESHOLD, and too short to
    // stack the detail pane below the list either — see s5_snapshot.rs's
    // identical geometry), the detail pane starts out absent, `Space` must
    // bring it up as a popup, a second `Space` must dismiss it, and `Esc`
    // must dismiss it too. This is the one seam s5_snapshot.rs's structural
    // tests can't cover: they build `BrowserState` directly and can set
    // `detail_popup_open` themselves, which would still pass even if the
    // `Char(' ')`/`Esc` key-handling arms in lib.rs were deleted. Only a
    // real keystroke against the compiled binary proves the wiring exists.
    //
    // Uses `screen_retry` (a reconstructed on-screen grid), not raw
    // `settle` output — `settle`'s string is the cumulative escape-code
    // stream, so a substring that appeared once (e.g. in the popup) stays
    // `contains`-true forever afterward even once ratatui's diff-redraw has
    // erased it on screen, which would make the "closed" assertions below
    // pass vacuously.
    //
    // Uses a scratch HOME, not bare `Session::spawn` — Space's detail-popup
    // toggle only does anything when `browser_state` is `Some`, and
    // `lib.rs`'s startup dispatch only populates that on `LastScreen::
    // Browser` (`Dashboard => (Screen::Dashboard, None)`). Bare `spawn`
    // inherits the ambient `$HOME`, so this test's real behavior depended on
    // whatever `last_screen` happened to be persisted in the machine
    // running it — passed reliably on a dev machine with a real
    // `~/.petridish/petri.toml` left at `last_screen = "browser"` from
    // manual testing, and failed deterministically in CI (a fresh `$HOME`,
    // defaulting to `LastScreen::Dashboard`) with a fully-painted Dashboard
    // frame that Space could never affect — not a timing race at all, this
    // was the exact ambient-prefs contamination `s7_pty.rs`'s module doc
    // comment already documents for a different test. Fix: isolate `$HOME`
    // (same convention as `s7_pty.rs`) and press `Tab` to reach the Browser
    // screen deterministically before testing Space, rather than depending
    // on any prefs file's default.
    let cols = 60u16;
    let rows = 10u16;
    let home = std::env::temp_dir().join(format!("petri_s5_pty_popup_home_{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("scratch home dir must be creatable");
    let mut session = Session::spawn_with_home(&fixture_path("normal.json"), cols, rows, &home);

    let initial_screen = settle_grid(&mut session, cols, rows).join("\n");
    assert!(
        initial_screen.contains("dashboard"),
        "petri must start on the Dashboard (S6 default) with a scratch HOME, got:\n{initial_screen}"
    );

    session
        .writer
        .write_all(b"\t")
        .expect("write Tab must succeed");
    let first_frame = settle_grid_until(&mut session, cols, rows, |grid| {
        grid.first().is_some_and(|row0| row0.contains("browser"))
    })
    .join("\n");
    assert!(
        first_frame.contains("browser"),
        "Tab must switch to the Browser screen before the popup toggle can be tested, got:\n{first_frame}"
    );
    assert!(
        !first_frame.contains("Branch:"),
        "detail pane must start absent at 60x10 (too narrow AND too short for either inline placement), got:\n{first_frame}"
    );

    session
        .writer
        .write_all(b" ")
        .expect("write 'Space' must succeed");
    let after_open = settle_grid_until(&mut session, cols, rows, |grid| {
        grid.iter().any(|line| line.contains("Branch:"))
    })
    .join("\n");
    assert!(
        after_open.contains("Branch:"),
        "Space must open the detail popup at 60x10, got:\n{after_open}"
    );

    session
        .writer
        .write_all(b" ")
        .expect("write 'Space' must succeed");
    let after_close = settle_grid_until(&mut session, cols, rows, |grid| {
        !grid.iter().any(|line| line.contains("Branch:"))
    })
    .join("\n");
    assert!(
        !after_close.contains("Branch:"),
        "a second Space must close the detail popup, got:\n{after_close}"
    );

    session
        .writer
        .write_all(b" ")
        .expect("write 'Space' must succeed");
    // Wait for the popup to actually be open again before sending Esc — not
    // just any settled frame. Without this, Esc could race ahead of the
    // reopen and dismiss nothing, and the assertion below would pass
    // vacuously (the exact class of trap this module's doc comment already
    // calls out for raw substring matching, just one keystroke later).
    let _ = settle_grid_until(&mut session, cols, rows, |grid| {
        grid.iter().any(|line| line.contains("Branch:"))
    });
    session
        .writer
        .write_all(&[0x1b])
        .expect("write 'Esc' must succeed");
    let after_esc = settle_grid_until(&mut session, cols, rows, |grid| {
        !grid.iter().any(|line| line.contains("Branch:"))
    })
    .join("\n");
    assert!(
        !after_esc.contains("Branch:"),
        "Esc must also close the detail popup, got:\n{after_esc}"
    );

    session
        .writer
        .write_all(b"q")
        .expect("write 'q' must succeed");
    let status = session.wait_with_timeout(Duration::from_secs(5));
    assert_eq!(
        status.exit_code(),
        0,
        "'q' must still exit 0 after toggling the detail popup"
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
