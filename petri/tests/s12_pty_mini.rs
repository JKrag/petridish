//! `SURF-7` mount #31, layer 3 (`petri/SPEC.md` §8, ADR-0003) — `petri --mini` driven
//! through a real pseudo-terminal against the compiled binary.
//!
//! **Added attended, after Phase D** (`PLAN-focus-panel.md` §9). This file matters more
//! than its sibling: `s12_mini_resolve.rs` grades `parse_args`, `resolve_mini` and
//! `MiniError`'s `Display` exhaustively as pure functions, but **nothing** grades
//! `run_mini` → `mini_poll_loop` → `render_mini_frame`. Phase D could have scored a
//! perfect 0 with no run loop at all. Everything below exists to close that gap:
//! alt-screen entry and exit, terminal restore on both quit keys, and the preflight
//! failures landing on a normal stderr rather than being swallowed by the alternate
//! screen (`SPEC.md` §4.4).
//!
//! `--mini`'s target is pinned by **name** in every test here rather than by cwd. The cwd
//! form is the default a user gets, but it makes the child's working directory
//! load-bearing, and a test whose result depends on where `cargo test` happened to be
//! invoked from is a fixture detail masquerading as an assertion. The one exception is
//! `a_cwd_outside_the_fleet_reports_not_a_project`, where that *is* the behaviour.

mod pty_support;
use pty_support::{Session, fixture_path};
use std::io::Write;
use std::time::Duration;

/// The `PROPOSAL-focus-panel.md` §3.2 corner-pane size: roomy enough for `--mini` to grow
/// its header and rule, which is what makes `petri · <name>` an assertable marker.
const COLS: u16 = 60;
const ROWS: u16 = 20;

fn scratch_home(tag: &str) -> std::path::PathBuf {
    let home =
        std::env::temp_dir().join(format!("petri_s12_pty_{tag}_home_{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("scratch home dir must be creatable");
    home
}

fn mini(tag: &str, cols: u16, rows: u16, args: &[&str]) -> Session {
    let home = scratch_home(tag);
    Session::spawn_with_args(&fixture_path("loaded.json"), cols, rows, Some(&home), args)
}

#[test]
fn a_pinned_pane_paints_the_panel_for_that_project() {
    // The run loop's first frame, which is the thing nothing else in the suite reaches.
    let mut session = mini("pinned", COLS, ROWS, &["--mini", "alpha-01"]);
    let screen = session.screen_until(
        COLS,
        ROWS,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
        |g| g.iter().any(|line| line.contains("alpha-01")),
    );

    assert!(
        screen[0].contains("petri") && screen[0].contains("alpha-01"),
        "--mini's header names the pinned project (PROPOSAL §3.2), got row 0: {:?}",
        screen[0]
    );
    assert!(
        !screen[0].contains("dashboard"),
        "--mini is not the Dashboard, got row 0: {:?}",
        screen[0]
    );
    assert!(
        screen.iter().any(|line| line.contains("agent")),
        "the panel's rungs must render, not just the chrome, got:\n{}",
        screen.join("\n")
    );
}

#[test]
fn q_exits_zero_from_a_mini_pane() {
    // Alt-screen enter *and* restore. A `--mini` pane that leaves the terminal in raw
    // mode is the failure mode this whole layer exists for.
    let mut session = mini("quit", COLS, ROWS, &["--mini", "alpha-01"]);
    let _ = session.screen_retry(
        COLS,
        ROWS,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
    );

    session.writer.write_all(b"q").expect("write q");
    let status = session.wait_with_timeout(Duration::from_secs(5));
    assert_eq!(status.exit_code(), 0, "'q' must exit 0 from --mini");
}

#[test]
fn esc_also_exits_zero_from_a_mini_pane() {
    // There is nothing for `Esc` to dismiss in a pane with no cursor, and a screen whose
    // only way out is a letter key is a trap in a tmux split — so it quits.
    let mut session = mini("esc", COLS, ROWS, &["--mini", "alpha-01"]);
    let _ = session.screen_retry(
        COLS,
        ROWS,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
    );

    session.writer.write_all(&[0x1b]).expect("write Esc");
    let status = session.wait_with_timeout(Duration::from_secs(5));
    assert_eq!(status.exit_code(), 0, "Esc must exit 0 from --mini");
}

#[test]
fn the_pane_survives_its_floor_geometry() {
    // 24x6 is the panel floor (`PROPOSAL` §3.4): no header, no rule, panel only. The
    // assertion is deliberately weak on content and strong on liveness — what a 24-column
    // pane elides is `s11_focus_plan.rs`'s business, whereas surviving the resize maths
    // is this layer's.
    let mut session = mini("floor", 24, 6, &["--mini", "alpha-01"]);
    let screen = session.screen_retry(24, 6, Duration::from_secs(5), Duration::from_millis(300), 5);
    assert!(
        screen.iter().any(|line| !line.trim().is_empty()),
        "the floor pane must paint something, got:\n{}",
        screen.join("\n")
    );

    session.writer.write_all(b"q").expect("write q");
    let status = session.wait_with_timeout(Duration::from_secs(5));
    assert_eq!(status.exit_code(), 0);
}

// ---------------------------------------------------------------------------
// The preflight failures — printed BEFORE the alternate screen (`SPEC.md` §4.4).
//
// These assert against the raw byte stream rather than a reconstructed grid, and that is
// correct here rather than sloppy: the whole claim is that these messages are *not* on a
// screen. `?1049h` (alt-screen entry) is asserted absent for exactly that reason.
// ---------------------------------------------------------------------------

fn raw(session: &mut Session) -> String {
    session.settle(Duration::from_secs(5), Duration::from_millis(300))
}

#[test]
fn an_unknown_pinned_name_reports_before_entering_the_alternate_screen() {
    let mut session = mini("unknown", COLS, ROWS, &["--mini", "no-such-project"]);
    let out = raw(&mut session);
    let status = session.wait_with_timeout(Duration::from_secs(5));

    assert_eq!(status.exit_code(), 1, "an unresolvable pin exits 1");
    assert!(
        out.contains("no-such-project") && out.contains("projects.json"),
        "the message must name the pin and the problem, got: {out:?}"
    );
    assert!(
        !out.contains("?1049h"),
        "the message must land on a normal stderr, never inside the alternate screen \
         (SPEC.md §4.4), got: {out:?}"
    );
}

#[test]
fn a_cwd_outside_the_fleet_reports_not_a_project() {
    // Bare `--mini`, so the cwd is the target. `cargo` runs an integration test with the
    // package root as the working directory, and no fixture project lives there — which
    // makes this deterministic without the test caring what the path actually is.
    let mut session = mini("cwd", COLS, ROWS, &["--mini"]);
    let out = raw(&mut session);
    let status = session.wait_with_timeout(Duration::from_secs(5));

    assert_eq!(status.exit_code(), 1);
    assert!(
        out.contains("not in projects.json"),
        "must name the problem, got: {out:?}"
    );
    assert!(
        out.contains("swab scan") && out.contains("petri --mini"),
        "must offer both ways out (PROPOSAL §8.3), got: {out:?}"
    );
    assert!(
        !out.contains("?1049h"),
        "never inside the alt screen: {out:?}"
    );
}

#[test]
fn a_missing_state_file_still_wins_over_a_mini_target() {
    // Ordering: the state file is checked first, so the user is told the daemon has never
    // run rather than that their project is unknown — which would be true but useless.
    let missing = std::env::temp_dir().join("petri_s12_pty_definitely_absent.json");
    let _ = std::fs::remove_file(&missing);
    let mut session = Session::spawn_with_args(&missing, COLS, ROWS, None, &["--mini", "alpha-01"]);
    let out = raw(&mut session);
    let status = session.wait_with_timeout(Duration::from_secs(5));

    assert_eq!(status.exit_code(), 1);
    assert!(
        out.contains("no state file at") && out.contains("swab scan"),
        "the shared swab message shape, got: {out:?}"
    );
    assert!(
        !out.contains("?1049h"),
        "never inside the alt screen: {out:?}"
    );
}

// ---------------------------------------------------------------------------
// The argv contract, as the binary actually parses it
// ---------------------------------------------------------------------------

#[test]
fn version_still_works_after_the_arg_parsing_rewrite() {
    // T7 moved argv handling out of `main.rs` into `parse_args`, widening `--version`
    // from "argv[1] only" to any position. `s12_mini_resolve.rs` pins the parse; this
    // pins that `main.rs` still acts on it — the two are separately breakable.
    let mut session = Session::spawn_with_args(
        &fixture_path("loaded.json"),
        COLS,
        ROWS,
        None,
        &["--version"],
    );
    let out = raw(&mut session);
    let status = session.wait_with_timeout(Duration::from_secs(5));

    assert_eq!(status.exit_code(), 0);
    assert!(
        out.contains("petri ") && out.contains(env!("CARGO_PKG_VERSION")),
        "--version must print the version even in a non-first position, got: {out:?}"
    );
    assert!(
        !out.contains("?1049h"),
        "and must not enter the alt screen: {out:?}"
    );
}

#[test]
fn an_unrecognised_flag_exits_two_with_a_usage_line() {
    // Exit 2 distinguishes "you typed it wrong" from exit 1's "the environment is not
    // ready", which is the difference between a fix in the shell and a fix in the daemon.
    let mut session =
        Session::spawn_with_args(&fixture_path("loaded.json"), COLS, ROWS, None, &["--focus"]);
    let out = raw(&mut session);
    let status = session.wait_with_timeout(Duration::from_secs(5));

    assert_eq!(status.exit_code(), 2, "a usage error exits 2, not 1");
    assert!(
        out.contains("--focus") && out.contains("usage:"),
        "must name the rejected argument and how to spell it right, got: {out:?}"
    );
}
