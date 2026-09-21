//! Regression for a bug reported against #88's `u` (token usage) action: `claude-monitor`
//! quits on Ctrl-C rather than `q`, and before this fix Ctrl-C took petri down with it.
//!
//! **Why.** `exec::run_in_terminal`'s `suspend()` re-enables the terminal's `ISIG` flag
//! (canonical mode) before the child runs, so Ctrl-C stops being a byte the child reads off
//! stdin and becomes a real `SIGINT` *signal* — delivered by the kernel to every process in
//! the terminal's foreground process group. Without `exec.rs`'s job-control fix
//! (`spawn_in_foreground`), the child shared petri's own group, so the same signal meant for
//! the child also hit petri, whose default disposition for `SIGINT` is termination. One
//! keystroke aimed at a stuck child took the whole TUI down.
//!
//! **Why a real PTY, not a unit test.** `SIGINT`'s foreground-process-group delivery is a
//! real-terminal, real-kernel behaviour — there is no `TestBackend` for a controlling
//! terminal's process-group bookkeeping. `s8_pty_handoff.rs`'s module doc comment states the
//! same reasoning for the suspend/resume round trip this test extends.
//!
//! **The stand-in for `claude-monitor`.** A shell script that never reads stdin and loops
//! forever — like a curses app whose whole job is redrawing on a timer, not one that exits
//! on its own. It prints a marker line once running so the test can wait for "the child has
//! actually reached its loop" rather than guessing with a fixed sleep (`petri/CLAUDE.md`'s
//! "never settle-then-assert" rule): the marker is plain text over an inherited stdout, so it
//! shows up in the PTY's raw byte stream the same way the real `claude-monitor` bug's
//! reproduction was confirmed by hand before this test existed.

mod pty_support;
use pty_support::{Session, fixture_path};
use std::io::Write;
use std::time::Duration;

/// A state file with a single project whose path is a directory that actually exists —
/// `Command::current_dir` fails outright on a missing one. Mirrors `s8_pty_handoff.rs`'s
/// helper of the same shape; not shared from there since that file's version is private to
/// its own module.
fn state_file_pointing_at(dir: &std::path::Path) -> std::path::PathBuf {
    let project_dir = dir.join("ctrlc-project");
    std::fs::create_dir_all(&project_dir).expect("project dir must be creatable");

    let text = std::fs::read_to_string(fixture_path("loaded.json"))
        .expect("the real fixture must be readable");
    let mut radar: serde_json::Value =
        serde_json::from_str(&text).expect("the real fixture must parse");

    let mut project = radar["projects"][0].clone();
    project["name"] = serde_json::json!("ctrlc-project");
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

/// A script that never quits on its own and never reads stdin — the shape that exposes this
/// bug. Its single argument (`launch_for`'s unknown-program branch always passes one) is
/// accepted and ignored.
fn write_hang_script(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("fake-claude-monitor.sh");
    std::fs::write(
        &path,
        b"#!/bin/sh\nprintf 'HANG_RUNNING\\n'\nwhile true; do sleep 0.2; done\n",
    )
    .expect("hang script must be writable");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("hang script must be made executable");
    }
    path
}

#[test]
fn ctrl_c_kills_the_stuck_child_alone_and_petri_survives() {
    let home = std::env::temp_dir().join(format!("petri_s88_ctrlc_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".petridish")).expect("scratch home must be creatable");

    let hang_script = write_hang_script(&home);

    // A path containing '/' is checked by executability directly (`exec::is_installed`),
    // not looked up on `PATH` — so the picker's "Other — specify path…" flow (a free-typed
    // absolute path in `[tools]`) resolves this candidate with no `PATH` plumbing at all.
    std::fs::write(
        home.join(".petridish").join("petri.toml"),
        format!(
            "last_screen = \"browser\"\ncollapsed = [false, false, true, true]\n\n[tools]\nusage = \"{}\"\n",
            hang_script.display()
        ),
    )
    .expect("seed prefs must be writable");

    let state_path = state_file_pointing_at(&home);
    let mut session = Session::spawn_with_home(&state_path, 90, 40, &home);

    let before = session.screen_until(
        90,
        40,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
        |grid| grid.iter().any(|r| r.contains("ctrlc-project")),
    );
    assert!(
        before.join("\n").contains("browser"),
        "petri must start on the Browser (seeded last_screen), got:\n{}",
        before.join("\n")
    );

    // `u` -> Terminal-mode hand-off to the hang script, which never exits on its own.
    session.writer.write_all(b"u").expect("write u");
    session.writer.flush().expect("flush");

    // Wait for the script's own marker in the raw stream, not a fixed sleep — this is the
    // "the child has actually reached its infinite loop" signal, and it also implicitly
    // proves the hand-off started (the marker can only appear once the child is running).
    let running = session.settle_until_raw(
        Duration::from_secs(5),
        Duration::from_millis(300),
        10,
        |stream| stream.contains("HANG_RUNNING"),
    );
    assert!(
        running,
        "the stuck child never printed its own marker — the hand-off likely never happened"
    );

    // Ctrl-C — the keystroke that used to take petri down with the child. Sent only after
    // the marker confirms the child is actually running, not merely spawned: `tcsetpgrp`'s
    // hand-off to the child's process group is a handful of syscalls, effectively
    // instantaneous, but the marker is the only externally observable proof it has happened
    // rather than an assumption about timing.
    session.writer.write_all(b"\x03").expect("write ctrl-c");
    session.writer.flush().expect("flush");

    // The script only ever exits on a signal (its `while true` loop has no other way out),
    // so `resume()` re-entering the alternate screen — the second `1049h` — is proof the
    // child actually died from the Ctrl-C, not proof of anything petri did on its own.
    let restored = session.settle_until_raw(
        Duration::from_secs(5),
        Duration::from_millis(400),
        8,
        |stream| Session::alt_screen_entries(stream) >= 2,
    );
    assert!(
        restored,
        "petri never re-entered the alternate screen after Ctrl-C — either the signal never \
         reached the child, or it took petri down with it (the bug this test guards)"
    );

    // The real proof petri survived: a clean, ordinary shutdown afterward. If Ctrl-C had
    // killed petri along with the child (the pre-fix bug), this `q` would either fail to
    // write (no process left to read it) or the wait below would time out.
    session.writer.write_all(b"q").expect("write q");
    session.writer.flush().expect("flush");
    let status = session.wait_with_timeout(Duration::from_secs(10));
    assert!(
        status.success(),
        "petri must still be alive and exit cleanly after Ctrl-C killed the stuck child, \
         got exit status {status:?}"
    );
}

/// Ctrl-Z regression, caught in review on top of the Ctrl-C fix above: `Child::wait()`
/// only ever returns on termination, never on a *stopped* child. `SIGTSTP` (Ctrl-Z) now
/// reaches only the child, same as `SIGINT` — but a stopped child is not a dead one, so
/// `spawn_in_foreground`'s wait would block on it forever, leaving petri hung with no way
/// out short of a second terminal and `kill`. The fix resumes a stopped child immediately
/// (petri has no `jobs`/`fg` of its own to bring one back with later) rather than truly
/// suspending it. Proven the same way as the Ctrl-C test: if this hangs, `q` below never
/// gets read and `wait_with_timeout` panics on a real timeout rather than a clean exit.
#[test]
fn ctrl_z_does_not_hang_petri_behind_a_stopped_child() {
    let home = std::env::temp_dir().join(format!("petri_s88_ctrlz_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".petridish")).expect("scratch home must be creatable");

    let hang_script = write_hang_script(&home);
    std::fs::write(
        home.join(".petridish").join("petri.toml"),
        format!(
            "last_screen = \"browser\"\ncollapsed = [false, false, true, true]\n\n[tools]\nusage = \"{}\"\n",
            hang_script.display()
        ),
    )
    .expect("seed prefs must be writable");

    let state_path = state_file_pointing_at(&home);
    let mut session = Session::spawn_with_home(&state_path, 90, 40, &home);

    session.screen_until(
        90,
        40,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
        |grid| grid.iter().any(|r| r.contains("ctrlc-project")),
    );

    session.writer.write_all(b"u").expect("write u");
    session.writer.flush().expect("flush");
    let running = session.settle_until_raw(
        Duration::from_secs(5),
        Duration::from_millis(300),
        10,
        |stream| stream.contains("HANG_RUNNING"),
    );
    assert!(running, "the stuck child never printed its own marker");

    // Ctrl-Z (0x1A, SIGTSTP) — before the fix, this is where petri would hang forever.
    session.writer.write_all(b"\x1a").expect("write ctrl-z");
    session.writer.flush().expect("flush");

    // Give the (fixed) resume-and-continue loop a moment to run its course, then prove
    // petri is still alive and responsive by actually finishing the job: Ctrl-C to end the
    // (still-running, never-stopped-for-good) child, then a clean `q`. Either step timing
    // out is the hang this test exists to catch.
    session.writer.write_all(b"\x03").expect("write ctrl-c");
    session.writer.flush().expect("flush");
    let restored = session.settle_until_raw(
        Duration::from_secs(5),
        Duration::from_millis(400),
        8,
        |stream| Session::alt_screen_entries(stream) >= 2,
    );
    assert!(
        restored,
        "petri never re-entered the alternate screen after Ctrl-Z then Ctrl-C — Ctrl-Z left \
         it hung behind a stopped child (the bug this test guards)"
    );

    session.writer.write_all(b"q").expect("write q");
    session.writer.flush().expect("flush");
    let status = session.wait_with_timeout(Duration::from_secs(10));
    assert!(
        status.success(),
        "petri must still be alive and exit cleanly after Ctrl-Z, got exit status {status:?}"
    );
}
