//! S8 acceptance gate: `MECH-2` actually round-trips the terminal
//! (petri/IDEAS.md `MECH-2`).
//!
//! The one part of the launcher that no unit test can reach — it disables raw
//! mode and leaves the alternate screen, which needs a real tty. Faking a
//! terminal for it would test the fake, not the hand-off.
//!
//! The child is `true`: it exits immediately, draws nothing, and exists on
//! every machine. That is deliberate. The thing under test is petri's
//! teardown/restore/`clear()` cycle, not any third-party TUI — and SPEC.md §8
//! is explicit that this PTY layer is the repo's flakiest, so depending on
//! `serie` being installed would make it flakier still for no added coverage.
//!
//! What proves it worked: petri is drawing its own Browser again afterwards.
//! Without invalidating ratatui's cached frame on the way back, ratatui diffs
//! against a frame it believes is still on screen and repaints almost nothing
//! — so a screen that still says "browser" after the hand-off is the
//! assertion. Mutation-probed: removing that invalidation turns this red.
//!
//! **What this test does NOT discriminate, stated rather than implied:**
//! commenting out the `suspend()` half leaves it green. `true` draws nothing,
//! so a child that never actually received the terminal is indistinguishable
//! from one that did. Covering that would need a child that paints, which
//! means either a third-party TUI (the dependency this layer must not take —
//! SPEC.md §8 calls this the flakiest layer in the repo) or a hand-rolled
//! ANSI-emitting script whose own output would then need parsing. The
//! restore half is the half that breaks in practice and the half that is
//! gated here; the suspend half is exercised for real every time a human
//! presses `g`.

mod pty_support;
use pty_support::{Session, fixture_path};
use std::io::Write;
use std::time::Duration;

/// A state file with a single project whose path is a directory that actually
/// exists — `Command::current_dir` fails outright on a missing one, so the
/// stock fixtures (which point at paths captured from another machine) would
/// exercise the failure path instead of the hand-off.
///
/// Derived from the real fixture rather than hand-written, so it cannot drift
/// out of step with the schema `petri` actually parses.
fn state_file_pointing_at(dir: &std::path::Path) -> std::path::PathBuf {
    let project_dir = dir.join("handoff-project");
    std::fs::create_dir_all(&project_dir).expect("project dir must be creatable");

    let text = std::fs::read_to_string(fixture_path("loaded.json"))
        .expect("the real fixture must be readable");
    let mut radar: serde_json::Value =
        serde_json::from_str(&text).expect("the real fixture must parse");

    let mut project = radar["projects"][0].clone();
    project["name"] = serde_json::json!("handoff-project");
    project["path"] = serde_json::json!(project_dir.to_string_lossy());
    project["git"]["github_url"] = serde_json::Value::Null;
    radar["projects"] = serde_json::json!([project]);

    let path = dir.join("radar.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&radar).expect("serialize"),
    )
    .expect("state file must be writable");
    path
}

#[test]
fn suspending_for_a_child_process_and_coming_back_leaves_a_usable_screen() {
    let home = std::env::temp_dir().join(format!("petri_s8_handoff_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".petridish")).expect("scratch home must be creatable");

    // Pre-answer the picker with a program the registry has never heard of, so
    // `resolve` takes the "Other — specify path…" branch: Terminal mode, one
    // argument. `true` ignores the argument and exits 0.
    std::fs::write(
        home.join(".petridish").join("petri.toml"),
        b"last_screen = \"browser\"\ncollapsed = [false, false, true, true]\n\n[tools]\ngitlog = \"true\"\n",
    )
    .expect("seed prefs must be writable");

    let state_path = state_file_pointing_at(&home);

    // The harness inherits this process's environment (PATH included), so
    // `true` is resolvable inside the child without any extra plumbing.
    let mut session = Session::spawn_with_home(&state_path, 90, 40, &home);

    let before = session.screen_until(
        90,
        40,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
        |grid| grid.iter().any(|r| r.contains("handoff-project")),
    );
    let before_body = before.join("\n");
    assert!(
        before_body.contains("browser"),
        "petri must start on the Browser (seeded last_screen), got:\n{before_body}"
    );
    assert!(
        before_body.contains("handoff-project"),
        "the generated state file must have loaded:\n{before_body}"
    );

    // `g` -> Terminal-mode hand-off to `true` -> immediate exit -> restore.
    session.writer.write_all(b"g").expect("write g");
    session.writer.flush().expect("flush");

    // `screen_until`, not `screen_retry`, and the predicate is the PROJECT NAME rather than
    // "browser" — both parts matter.
    //
    // `screen_retry` only retries an all-blank grid, and the grid this race produces is not
    // blank: `g` leaves the alternate screen, runs the child, re-enters and repaints, so a
    // settle window landing mid-cycle catches a *partially* painted frame. Measured under
    // load before this changed: 6 failures in 20 runs, the worst flake in the suite.
    //
    // "browser" would be the wrong thing to wait for even so — it is in the header of the
    // pre-keystroke frame too, so it cannot distinguish "repainted" from "not started
    // yet". The project list is what the hand-off has to restore, so it is what to wait for.
    let after = session.screen_until(
        90,
        40,
        Duration::from_secs(10),
        Duration::from_millis(400),
        6,
        |grid| grid.iter().any(|r| r.contains("handoff-project")),
    );
    let after_body = after.join("\n");
    assert!(
        after_body.contains("browser"),
        "petri did not repaint after the hand-off — without invalidating the \
         cached frame, ratatui keeps diffing against one the child overwrote:\n{after_body}"
    );
    assert!(
        after_body.contains("handoff-project"),
        "the project list must be back on screen after the hand-off:\n{after_body}"
    );

    // Do NOT send `q` until petri owns the terminal again.
    //
    // This is the failure this test actually had, and it did not look like a race at all:
    // it surfaced as "child did not exit within 10s", ten seconds after the real damage.
    // While the hand-off's child holds the terminal, raw mode is off, so a byte written
    // here is buffered by the line discipline in canonical mode and discarded when petri
    // restores raw mode. The `q` was simply never delivered.
    //
    // The screen cannot be used to detect this. Both assertions above — "browser" and the
    // project name — are equally true of the frame from *before* `g` was pressed, because
    // the hand-off restores the very same screen it left. The second alternate-screen entry
    // is the signal that has no such ambiguity.
    let restored = session.settle_until_raw(
        Duration::from_secs(10),
        Duration::from_millis(400),
        8,
        |stream| Session::alt_screen_entries(stream) >= 2,
    );
    assert!(
        restored,
        "petri never re-entered the alternate screen after the hand-off — it did not take \
         the terminal back, so no keystroke after this point could be delivered"
    );

    session.writer.write_all(b"q").expect("write q");
    session.writer.flush().expect("flush");
    session.wait_with_timeout(Duration::from_secs(10));
}
