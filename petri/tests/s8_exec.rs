//! S8 gate for the launcher (petri/IDEAS.md `MECH-3`).
//!
//! `spawn_detached` is testable here because it never touches the terminal —
//! that is the whole point of the mode. Its `MECH-2` sibling
//! `run_in_terminal` deliberately is NOT tested here: it disables raw mode and
//! leaves the alternate screen, which requires a real tty, so it is covered
//! end-to-end from the PTY layer once a key is bound to it. Faking a terminal
//! for it would test the fake, not the hand-off.

use petri::exec::{self, Outcome};
use petri::tools::{ExecMode, Launch};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("petri_s8_exec_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir must be creatable");
    dir
}

/// Detached means we are explicitly not waiting, so the observable effect
/// arrives asynchronously. Poll for it rather than sleeping a fixed amount —
/// a fixed sleep is the classic source of a flaky test on a loaded machine.
fn wait_for(path: &std::path::Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn launch(program: &str, args: &[&str]) -> Launch {
    Launch {
        program: program.to_string(),
        args: args.iter().map(|a| (*a).to_string()).collect(),
        mode: ExecMode::Background,
    }
}

#[test]
fn spawn_detached_actually_starts_the_program() {
    let dir = scratch_dir("spawn_detached_actually_starts_the_program");
    let marker = dir.join("ran");
    let out = exec::spawn_detached(&launch("sh", &["-c", "touch ran"]), &dir);
    assert!(
        matches!(out, Outcome::Detached),
        "expected Detached, got {out:?}"
    );
    assert!(wait_for(&marker), "the child never ran: {marker:?} was not created");
}

#[test]
fn spawn_detached_runs_in_the_given_working_directory() {
    // Not incidental: `serie` and `tig` find the repository from the working
    // directory alone, and an editor opened in the wrong cwd is worse than no
    // editor. The cwd is part of the contract, not a convenience.
    let dir = scratch_dir("spawn_detached_runs_in_the_given_working_directory");
    let marker = dir.join("where");
    exec::spawn_detached(&launch("sh", &["-c", "pwd > where"]), &dir);
    assert!(wait_for(&marker), "child never wrote its cwd");

    let got = std::fs::read_to_string(&marker).expect("marker must be readable");
    let got = std::fs::canonicalize(got.trim()).expect("child cwd must resolve");
    let want = std::fs::canonicalize(&dir).expect("scratch dir must resolve");
    assert_eq!(got, want);
}

#[test]
fn spawn_detached_reports_a_missing_program_instead_of_panicking() {
    // The stale-tool case: a stored `[tools]` choice that was uninstalled
    // between resolution and launch. It must surface as a value the caller can
    // render, never as a panic that takes the TUI down with it.
    let dir = scratch_dir("spawn_detached_reports_a_missing_program_instead_of_panicking");
    let out = exec::spawn_detached(
        &launch("petri-definitely-not-a-real-program-9f3a", &[]),
        &dir,
    );
    match out {
        Outcome::Failed(e) => assert_eq!(
            e.kind(),
            std::io::ErrorKind::NotFound,
            "expected a not-found error, got {e:?}"
        ),
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn spawn_detached_passes_its_arguments_through() {
    let dir = scratch_dir("spawn_detached_passes_its_arguments_through");
    let marker = dir.join("args");
    exec::spawn_detached(
        &launch("sh", &["-c", "printf '%s' \"$0 $1\" > args", "alpha", "beta"]),
        &dir,
    );
    assert!(wait_for(&marker), "child never wrote its args");
    assert_eq!(
        std::fs::read_to_string(&marker).expect("readable"),
        "alpha beta"
    );
}
