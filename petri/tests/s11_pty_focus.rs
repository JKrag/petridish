//! `SURF-8` mount #30, layer 3 (`petri/SPEC.md` §8, ADR-0003) — the Dashboard's focus
//! popup driven through a real pseudo-terminal against the compiled binary.
//!
//! **Added attended, after Phase D**, which is `PLAN-focus-panel.md` §9's explicit rule:
//! an unattended loop cannot tell a PTY flake from a defect, so it either halts on a false
//! failure or learns to ignore the layer. Everything here is therefore a *lifecycle and
//! wiring* check — that `lib.rs` actually routes `Space`/`Esc` into `press_space`/
//! `close_focus` and that the overlay reaches the screen — not a re-assertion of the
//! contract, which `s11_focus_mount.rs` already pins as pure state with no terminal
//! involved.
//!
//! All assertions go through `Session::screen_until`, never raw substring matching on the
//! byte stream: `pty_support`'s doc comment records that as the root cause of the Python
//! TUI's worst CI flakiness, and `screen_until` specifically exists for the
//! post-keystroke case here — a settle window can return a perfectly well-formed grid
//! that is simply the frame *before* the key was processed.
//!
//! `" Focus "` is the marker throughout. It is the popup block's title and appears
//! nowhere in `dashboard.rs`'s own chrome, so its presence is unambiguous.

mod pty_support;
use pty_support::{Session, fixture_path};
use std::io::Write;
use std::time::Duration;

const COLS: u16 = 80;
const ROWS: u16 = 40;

/// A scratch `HOME` per test, so a test that presses `Space` can never write through to
/// the real `~/.petridish/petri.toml` — `s7_pty.rs` records that isolation bug being
/// found the hard way, against a developer's actual preferences file.
fn scratch_home(tag: &str) -> std::path::PathBuf {
    let home =
        std::env::temp_dir().join(format!("petri_s11_pty_{tag}_home_{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("scratch home dir must be creatable");
    home
}

fn settled(session: &mut Session, cols: u16, rows: u16) -> Vec<String> {
    session.screen_retry(
        cols,
        rows,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
    )
}

fn until(
    session: &mut Session,
    cols: u16,
    rows: u16,
    predicate: impl FnMut(&[String]) -> bool,
) -> Vec<String> {
    session.screen_until(
        cols,
        rows,
        Duration::from_secs(2),
        Duration::from_millis(300),
        5,
        predicate,
    )
}

fn has_focus_title(grid: &[String]) -> bool {
    grid.iter().any(|line| line.contains("Focus"))
}

#[test]
fn space_on_a_project_row_opens_the_focus_popup() {
    // The whole point of T5's `lib.rs` rewiring, end to end. `j` first: petri lands with
    // the cursor on the RUNNING header, and `Space` there is the *unchanged* binding.
    let home = scratch_home("open");
    let mut session = Session::spawn_with_home(&fixture_path("loaded.json"), COLS, ROWS, &home);
    let initial = settled(&mut session, COLS, ROWS);
    assert!(
        initial[0].contains("petri") && initial[0].contains("dashboard"),
        "precondition: petri starts on the Dashboard, got row 0: {:?}",
        initial[0]
    );
    assert!(
        !has_focus_title(&initial),
        "precondition: no popup before any keystroke, got:\n{}",
        initial.join("\n")
    );

    session.writer.write_all(b"j").expect("write j");
    let _ = settled(&mut session, COLS, ROWS);
    session.writer.write_all(b" ").expect("write Space");

    let opened = until(&mut session, COLS, ROWS, has_focus_title);
    assert!(
        has_focus_title(&opened),
        "Space on a project row must open the focus popup (issue #30), got:\n{}",
        opened.join("\n")
    );
}

#[test]
fn esc_closes_the_focus_popup() {
    let home = scratch_home("esc");
    let mut session = Session::spawn_with_home(&fixture_path("loaded.json"), COLS, ROWS, &home);
    let _ = settled(&mut session, COLS, ROWS);
    session.writer.write_all(b"j").expect("write j");
    let _ = settled(&mut session, COLS, ROWS);
    session.writer.write_all(b" ").expect("write Space");
    let opened = until(&mut session, COLS, ROWS, has_focus_title);
    assert!(
        has_focus_title(&opened),
        "precondition: the popup is open, got:\n{}",
        opened.join("\n")
    );

    session.writer.write_all(&[0x1b]).expect("write Esc");

    let closed = until(&mut session, COLS, ROWS, |g| !has_focus_title(g));
    assert!(
        !has_focus_title(&closed),
        "Esc must close the popup, got:\n{}",
        closed.join("\n")
    );
    assert!(
        closed[0].contains("dashboard"),
        "and must leave the Dashboard painted underneath, got row 0: {:?}",
        closed[0]
    );
}

#[test]
fn space_twice_on_a_row_closes_what_it_opened() {
    let home = scratch_home("toggle");
    let mut session = Session::spawn_with_home(&fixture_path("loaded.json"), COLS, ROWS, &home);
    let _ = settled(&mut session, COLS, ROWS);
    session.writer.write_all(b"j").expect("write j");
    let _ = settled(&mut session, COLS, ROWS);

    session.writer.write_all(b" ").expect("write Space");
    let opened = until(&mut session, COLS, ROWS, has_focus_title);
    assert!(has_focus_title(&opened), "precondition: popup open");

    session.writer.write_all(b" ").expect("write Space again");
    let closed = until(&mut session, COLS, ROWS, |g| !has_focus_title(g));
    assert!(
        !has_focus_title(&closed),
        "a second Space on a row closes the popup, got:\n{}",
        closed.join("\n")
    );
}

#[test]
fn space_on_a_header_still_collapses_the_section() {
    // The binding §7 leaves alone, and the one a wrong `press_space` branch would break
    // most visibly. petri lands on the RUNNING header, so no navigation is needed —
    // that is also what makes this the complement of the `j`-first tests above.
    let home = scratch_home("header");
    let mut session = Session::spawn_with_home(&fixture_path("loaded.json"), COLS, ROWS, &home);
    let initial = settled(&mut session, COLS, ROWS);
    let rows_before = initial
        .iter()
        .filter(|line| line.contains("alpha-"))
        .count();
    assert!(
        rows_before > 0,
        "precondition: RUNNING must be showing project rows, got:\n{}",
        initial.join("\n")
    );

    session.writer.write_all(b" ").expect("write Space");

    let collapsed = until(&mut session, COLS, ROWS, |g| {
        g.iter().filter(|line| line.contains("alpha-")).count() < rows_before
    });
    assert!(
        collapsed
            .iter()
            .filter(|line| line.contains("alpha-"))
            .count()
            < rows_before,
        "Space on the RUNNING header must still collapse it, got:\n{}",
        collapsed.join("\n")
    );
    assert!(
        !has_focus_title(&collapsed),
        "and must NOT open the focus popup, got:\n{}",
        collapsed.join("\n")
    );
}

#[test]
fn a_small_terminal_renders_the_panel_full_screen_instead_of_a_popup() {
    // `PROPOSAL-focus-panel.md` §10's fallback, which `focus_placement` decides and
    // `s11_focus_mount.rs` grades as arithmetic — this is the half that proves the mount
    // actually honours the decision. At 48x14 there is no border and no title, so the
    // discriminator is that the Dashboard's own header is gone: the panel took the frame.
    const W: u16 = 48;
    const H: u16 = 14;
    let home = scratch_home("fullscreen");
    let mut session = Session::spawn_with_home(&fixture_path("loaded.json"), W, H, &home);
    let initial = settled(&mut session, W, H);
    assert!(
        initial[0].contains("dashboard"),
        "precondition: the Dashboard header is on row 0, got: {:?}",
        initial[0]
    );

    session.writer.write_all(b"j").expect("write j");
    let _ = settled(&mut session, W, H);
    session.writer.write_all(b" ").expect("write Space");

    let full = until(&mut session, W, H, |g| !g[0].contains("dashboard"));
    assert!(
        !full[0].contains("dashboard"),
        "at 48x14 the panel must take the whole frame, not overlay it, got:\n{}",
        full.join("\n")
    );
    assert!(
        !has_focus_title(&full),
        "and the full-screen render carries no popup border/title, got:\n{}",
        full.join("\n")
    );
    // Without this the test passes on a blank grid too — a crashed binary paints no
    // "dashboard" either, which is exactly the vacuous pass `SPEC.md` §8 warns about.
    assert!(
        full.iter().any(|line| line.contains("alpha-")),
        "the panel itself must have rendered its project, got:\n{}",
        full.join("\n")
    );
}

#[test]
fn q_still_exits_zero_after_the_focus_gesture() {
    // The lifecycle check: an overlay that leaves the terminal in raw mode or wedges the
    // loop is the failure this layer exists to catch, and no state test can see it.
    let home = scratch_home("quit");
    let mut session = Session::spawn_with_home(&fixture_path("loaded.json"), COLS, ROWS, &home);
    let _ = settled(&mut session, COLS, ROWS);

    for keys in [&b"j"[..], b" ", b"j", b"k", &[0x1b], b" "] {
        session
            .writer
            .write_all(keys)
            .unwrap_or_else(|e| panic!("write {keys:?} must succeed: {e}"));
        let _ = session.settle(Duration::from_millis(500), Duration::from_millis(150));
        assert!(
            session.child.try_wait().ok().flatten().is_none(),
            "petri must still be alive after sending {keys:?}"
        );
    }

    session.writer.write_all(b"q").expect("write q");
    let status = session.wait_with_timeout(Duration::from_secs(5));
    assert_eq!(
        status.exit_code(),
        0,
        "'q' must exit 0 after a full open/navigate/close focus-popup gesture"
    );
}
