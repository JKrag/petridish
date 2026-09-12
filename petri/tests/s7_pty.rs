//! S7 acceptance gate, layer 3 (petri/SPEC.md §3.2/§6/§8, ADR-0003) —
//! protected, authored by the orchestrator, not the delegate. Real keystrokes
//! against the compiled `petri` binary via the shared `pty_support` harness.
//!
//! Both remaining tests wait on `Session::screen_until(...)` for a header
//! badge rather than a bare `settle`-then-assert or raw substring matching
//! against the byte stream — the preferences warning printed before raw mode
//! (`lib.rs`'s Step 1.5) can make the PTY nonblank before the first real
//! frame, so a blanket retry-until-nonblank primitive like `screen_retry`
//! isn't enough here; see `pty_support`'s doc comment on `screen`/`screen_until`
//! for why petri/SPEC.md §8 calls this out explicitly as the fix for the
//! Python TUI's worst historical CI flakiness.
//!
//! `tab_switches_dashboard_to_browser_and_back` used to live here too — `Tab`
//! round-tripping Dashboard→Browser→Dashboard and persisting each switch —
//! but that is exactly `handle_key`'s dispatch, not `run`'s own startup
//! wiring, so issue #61 moved it to `s61_key_dispatch.rs` once `handle_key`
//! took `prefs_path` as a parameter (the scratch-`$HOME` isolation this test
//! used to need for a spawned subprocess). What's left here is what `Tab`'s
//! dispatch genuinely cannot prove on its own: that `run` itself reads
//! `petri.toml` before the alternate screen is entered.
//!
//! `corrupt_petri_toml_does_not_prevent_startup` passes even against the
//! CURRENT stub (prefs aren't wired into startup at all yet, so Dashboard's
//! hardcoded S6 defaults render regardless of any file content — which are
//! identical to what a correctly-defaulted prefs file would produce). Kept
//! as a lifecycle/regression check (same convention as S5's PTY layer), not
//! the discriminating one — that's `valid_petri_toml_is_applied_on_startup`,
//! which uses a non-default persisted `last_screen` to force a visible
//! difference no hardcoded default can coincidentally satisfy.
//!
//! Requires `lib.rs`'s `poll_loop` to route `Tab` into a Dashboard<->Browser
//! screen switch, and `prefs::load`/`default_prefs_path` to be wired in at
//! startup (not `todo!()`) — confirmed failing against the current stub
//! before delegating S7.
//!
//! `tab_switch_persists_through_the_real_binary` is the caller-level seam
//! `s61_key_dispatch.rs`'s `switching_screens_preserves_stored_tool_choices`
//! cannot be: that test calls `handle_key` directly with an explicit
//! `prefs_path`, so it proves the dispatch logic but not that `run` itself
//! still resolves `prefs::default_prefs_path()` and threads it all the way
//! through `poll_loop` correctly. A regression in that wiring (the wrong path
//! passed, or `default_prefs_path()` stopped being called at all) would still
//! pass every in-process dispatch test.

mod pty_support;
use pty_support::{BROWSER_HEADER, DASHBOARD_HEADER, Session, fixture_path};
use std::io::Write;
use std::time::Duration;

#[test]
fn valid_petri_toml_is_applied_on_startup() {
    // The one PTY assertion that actually discriminates "prefs wired into
    // startup" from "not wired at all": `corrupt_petri_toml_does_not_prevent_startup`
    // below happens to pass even against the CURRENT stub (prefs::load is
    // never called yet, so Dashboard's own hardcoded defaults render — which
    // are IDENTICAL to what a correctly-defaulted prefs file would produce).
    // A valid, non-default prefs file has no such ambiguity: if `last_screen
    // = "browser"` is ever actually read and applied, startup must land on
    // the Browser instead of S6's hardcoded Dashboard default — confirmed
    // failing against the current stub before delegating S7.
    let home = std::env::temp_dir().join(format!(
        "petri_s7_pty_valid_toml_home_{}",
        std::process::id()
    ));
    let petridish_dir = home.join(".petridish");
    std::fs::create_dir_all(&petridish_dir).expect("scratch .petridish dir must be creatable");
    std::fs::write(
        petridish_dir.join("petri.toml"),
        b"last_screen = \"browser\"\ncollapsed = [true, true, true, true]\n",
    )
    .expect("write valid petri.toml must succeed");

    let mut session = Session::spawn_with_home(&fixture_path("normal.json"), 80, 24, &home);
    // Not `screen_retry`: it retries only an all-blank grid, and petri's prefs warning on
    // stderr makes the output non-blank before any frame exists.
    let screen = session.screen_until(
        80,
        24,
        Duration::from_secs(5),
        Duration::from_millis(300),
        6,
        |grid| grid.iter().any(|r| r.contains("petri · browser")),
    );
    assert!(
        screen[0].contains("petri") && screen[0].contains("browser"),
        "a persisted last_screen = \"browser\" must be applied on startup, got row 0: {:?}",
        screen[0]
    );

    session
        .writer
        .write_all(b"q")
        .expect("write 'q' must succeed");
    let status = session.wait_with_timeout(Duration::from_secs(5));
    assert_eq!(
        status.exit_code(),
        0,
        "'q' must still exit 0 after starting from a persisted Browser screen"
    );
}

#[test]
fn corrupt_petri_toml_does_not_prevent_startup() {
    // petri/SPEC.md §6: "A corrupt or unparseable file means defaults plus a
    // warning — never a crash, and never a refusal to start. There is a test
    // for this." This is that test, at the real-binary level (the pure-state
    // half — load() itself not panicking — is covered by s7_prefs.rs).
    let home = std::env::temp_dir().join(format!(
        "petri_s7_pty_corrupt_toml_home_{}",
        std::process::id()
    ));
    let petridish_dir = home.join(".petridish");
    std::fs::create_dir_all(&petridish_dir).expect("scratch .petridish dir must be creatable");
    std::fs::write(
        petridish_dir.join("petri.toml"),
        b"not valid toml { [[[ ===",
    )
    .expect("write corrupt petri.toml must succeed");

    let mut session = Session::spawn_with_home(&fixture_path("normal.json"), 80, 24, &home);
    // A corrupt prefs file means defaults, so the Dashboard is what must appear — and the
    // warning it also prints is exactly what makes a blank-only retry useless here.
    let screen = session.screen_until(
        80,
        24,
        Duration::from_secs(5),
        Duration::from_millis(300),
        6,
        |grid| grid.iter().any(|r| r.contains("petri · dashboard")),
    );
    let whole = screen.join("\n");
    assert!(
        whole.contains("petri") && whole.contains("dashboard"),
        "petri must still start normally (defaults, not a crash) with a corrupt petri.toml, got:\n{whole}"
    );

    session
        .writer
        .write_all(b"q")
        .expect("write 'q' must succeed");
    let status = session.wait_with_timeout(Duration::from_secs(5));
    assert_eq!(
        status.exit_code(),
        0,
        "'q' must still exit 0 with a corrupt petri.toml present"
    );
}

#[test]
fn tab_switch_persists_through_the_real_binary() {
    // Seed a `[tools]` table the same way the deleted `s8_pty_prefs.rs`'s
    // `switching_screens_preserves_stored_tool_choices` used to — this is the
    // exact regression that test caught (adding a field to `Prefs` without
    // updating every `Prefs { .. }` literal in `lib.rs` silently wrote an
    // empty `[tools]` on the next save) proven here through `run`'s own
    // `default_prefs_path()` wiring rather than a directly-passed path.
    let home = std::env::temp_dir().join(format!(
        "petri_s7_pty_tab_persist_home_{}",
        std::process::id()
    ));
    let petridish_dir = home.join(".petridish");
    std::fs::create_dir_all(&petridish_dir).expect("scratch .petridish dir must be creatable");
    std::fs::write(
        petridish_dir.join("petri.toml"),
        b"last_screen = \"dashboard\"\ncollapsed = [false, false, true, true]\n\n[tools]\nedit = \"code\"\ngitlog = \"serie\"\n",
    )
    .expect("write petri.toml must succeed");

    let mut session = Session::spawn_with_home(&fixture_path("normal.json"), 80, 24, &home);
    session.screen_until(
        80,
        24,
        Duration::from_secs(5),
        Duration::from_millis(300),
        6,
        |grid| grid.iter().any(|r| r.contains(DASHBOARD_HEADER)),
    );

    session
        .writer
        .write_all(b"\t")
        .expect("write Tab must succeed");
    let screen = session.screen_until(
        80,
        24,
        Duration::from_secs(5),
        Duration::from_millis(300),
        6,
        |grid| grid.iter().any(|r| r.contains(BROWSER_HEADER)),
    );
    assert!(
        screen.iter().any(|r| r.contains("browser")),
        "precondition: Tab must switch to the Browser through the real binary"
    );

    session
        .writer
        .write_all(b"q")
        .expect("write 'q' must succeed");
    let status = session.wait_with_timeout(Duration::from_secs(5));
    assert_eq!(status.exit_code(), 0, "'q' must still exit 0 after Tab");

    let written = std::fs::read_to_string(petridish_dir.join("petri.toml"))
        .expect("prefs file must still exist after the real binary's Tab-triggered save");
    assert!(
        written.contains("[tools]") && written.contains("edit") && written.contains("code"),
        "the real run()->poll_loop()->handle_key() wiring must persist through \
         default_prefs_path(), not just handle_key() called directly, got:\n{written}"
    );
}
