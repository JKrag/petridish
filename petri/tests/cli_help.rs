//! `--help` / `-h` at the process boundary (issue #42).
//!
//! Why this is a process test and deliberately **not** a PTY test: the claims here are
//! about what the binary writes to which stream and what it exits with, plus the one
//! property `parse_args` unit tests cannot reach — that asking for help never enters the
//! alternate screen. A pipe is enough to show all of that, and #48's question ("does this
//! assert something only a real terminal can show?") answers *no* for every assertion
//! below, so none of them should pay for a pseudo-terminal or its timing.
//!
//! The contract itself lives in `parse_args`' doc comment and is asserted case by case in
//! `s12_mini_resolve.rs`; this file only pins the wiring in `main.rs`.

use std::process::Command;

/// Run the real binary with `args`, returning (exit code, stdout, stderr).
fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_petri"))
        .args(args)
        .output()
        .expect("spawn petri");
    (
        out.status.code().expect("petri exited via a signal"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn help_prints_usage_to_stdout_and_exits_zero() {
    for flag in ["--help", "-h"] {
        let (code, stdout, _stderr) = run(&[flag]);
        assert_eq!(code, 0, "{flag} must exit 0");
        assert!(
            stdout.contains("usage:") && stdout.contains("--mini"),
            "{flag} must print the usage to stdout, got: {stdout:?}"
        );
    }
}

#[test]
fn help_goes_to_stdout_not_stderr() {
    // `petri --help | less` must not pipe an empty stream. Help was asked for, so it is
    // this run's output; the parse-error path is the one that belongs on stderr.
    let (_code, stdout, stderr) = run(&["--help"]);
    assert!(!stdout.is_empty(), "stdout must carry the help");
    assert!(
        !stderr.contains("usage:"),
        "the help must not be duplicated onto stderr, got: {stderr:?}"
    );
}

#[test]
fn help_never_enters_the_alternate_screen() {
    // The property the unit tests cannot reach, and the reason this file spawns a process
    // at all: a `--help` that fell through into the TUI would still print the right text
    // and still exit 0, so only the absence of the terminal-setup escape sequences
    // distinguishes "printed help and left" from "printed help and took over the screen".
    let (_code, stdout, stderr) = run(&["--help"]);
    for (name, stream) in [("stdout", &stdout), ("stderr", &stderr)] {
        assert!(
            !stream.contains("\x1b[?1049h"),
            "{name} contains an alternate-screen entry: --help must not start the TUI"
        );
    }
}

#[test]
fn help_outranks_version_at_the_process_boundary() {
    // Rule 0, asserted through `main.rs` rather than through `parse_args` alone — the two
    // branches are adjacent in `main` and the wrong order there is invisible to a unit test
    // of the parser.
    let (code, stdout, _stderr) = run(&["--version", "--help"]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("usage:"),
        "help must win over --version, got: {stdout:?}"
    );
}

#[test]
fn a_parse_error_still_goes_to_stderr_and_exits_two() {
    // The other half of the stream split: this must not have been collapsed into the help
    // path while wiring it up.
    let (code, stdout, stderr) = run(&["--focus"]);
    assert_eq!(code, 2, "an unrecognised flag must exit 2");
    assert!(
        stderr.contains("--focus"),
        "the error belongs on stderr, got: {stderr:?}"
    );
    assert!(
        stdout.is_empty(),
        "nothing belongs on stdout for a parse error, got: {stdout:?}"
    );
}
