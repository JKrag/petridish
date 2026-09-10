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
    let _ = session.wait_with_timeout(EXIT_BUDGET);

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
    wait_until_ready(&mut session, 80, 24);

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
    let status = session.wait_with_timeout(EXIT_BUDGET);
    assert_eq!(
        status.exit_code(),
        0,
        "'q' must still exit 0 after a sequence of navigation/filter keystrokes"
    );
}

/// Wait until petri has actually taken the terminal and painted, before sending it a key.
///
/// A bare `settle` returns as soon as the stream is quiet for 300ms, which can be *before*
/// the binary has entered raw mode. A key written then is buffered by the line discipline
/// in canonical mode and discarded when raw mode is enabled — the same mechanism that made
/// `s8_pty_handoff` lose its `q`, but at startup rather than after a hand-off. It surfaces
/// the same way too: not as a wrong frame, but as "child did not exit", seconds later.
/// Measured at 8 failures in 24 runs at eight-way concurrency on an idle machine.
///
/// The predicate is the header BADGE, `"petri \u{b7} "`, not a bare `"petri"`. The bare form
/// does not state the condition above at all: `prefs::load` warns `petri S7: preferences
/// file ... missing or unreadable` and lib.rs's "Step 1.5" emits it deliberately before
/// `enable_raw_mode`, so that text is on the reconstructed grid while the terminal is still
/// in canonical mode — exactly the window this function exists to wait past. The badge is
/// painted, so it cannot appear until after the alternate-screen entry that follows raw
/// mode. Either screen's badge will do (`" petri \u{b7} dashboard "` / `" petri \u{b7} browser "`),
/// since this only has to know that petri owns the terminal, not which screen it is on.
fn wait_until_ready(session: &mut Session, cols: u16, rows: u16) {
    session.screen_until(
        cols,
        rows,
        Duration::from_secs(5),
        Duration::from_millis(300),
        10,
        |grid| grid.iter().any(|r| r.contains("petri \u{b7} ")),
    );
}

/// The exit budget is a HANG DETECTOR, not a performance assertion. Five seconds is tight
/// enough to trip on a busy machine, and a test that fails because the box was loaded
/// teaches people to re-run rather than to look.
const EXIT_BUDGET: Duration = Duration::from_secs(20);

/// Settle until the grid actually shows what the caller is about to assert on.
///
/// The unconditional `settle_grid` this file used to carry alongside is GONE, not kept for
/// convenience, because its contract was itself the bug — the same verdict `pty_support`'s
/// module doc records for `spawn_and_settle_nonempty`. It wrapped `screen_retry`, which
/// retries only on a BLANK grid: a still-painted previous frame (the redraw for the
/// keystroke just sent hasn't happened yet) is not blank and came back as-is, and neither
/// is the pre-alt-screen grid holding only the "preferences file ... missing" warning. Both
/// failed the following assertion on a frame petri was about to paint correctly. Retries
/// until `predicate` holds or the attempt budget is spent, so a genuine regression still
/// fails loudly.
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

    // `settle_grid_until`, not bare `settle_grid`, for the same reason `wait_until_ready`
    // exists: `screen_retry` retries only an all-*blank* grid, and the grid before petri
    // takes the terminal is not blank — the scratch HOME has no `petri.toml`, so
    // `prefs::load`'s "preferences file ... missing" warning is on it, printed to a normal
    // stderr before `enable_raw_mode` (lib.rs's "Step 1.5"). `settle_grid` happily returned
    // that, and the assertion below then failed on a frame petri had not painted yet.
    // Measured after the rest of this PR's conversion: 1 failure in 24 runs at eight-way
    // concurrency, the last one left in the suite.
    let initial_screen = settle_grid_until(&mut session, cols, rows, |grid| {
        grid.iter().any(|r| r.contains("dashboard"))
    })
    .join("\n");
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
    let status = session.wait_with_timeout(EXIT_BUDGET);
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
    wait_until_ready(&mut session, 80, 24);
    session
        .writer
        .write_all(b"q")
        .expect("write 'q' must succeed");
    let status = session.wait_with_timeout(EXIT_BUDGET);
    assert_eq!(
        status.exit_code(),
        0,
        "'q' must still exit 0 with the Browser wired in"
    );
}
