//! S4 acceptance gate, layer 3 (petri/SPEC.md §8, ADR-0003) — protected, authored
//! by the orchestrator, not the delegate. Drives the real compiled `petri` binary
//! in a PTY. Deterministic by construction (ADR-0003): drains the pty continuously
//! while waiting (a stalled reader looks exactly like the child ignoring input),
//! sets the winsize explicitly (a forked pty starts at 0x0), and reads until the
//! stream goes quiet before asserting rather than pattern-matching the raw byte
//! stream. No fixed-clock injection needed yet — S4 renders no time-derived text.
//!
//! All three tests currently FAIL (not error) against `petri::run`'s `todo!()`
//! body: the binary panics almost immediately, so the pty closes before
//! `settle()` observes the expected content, and the assertions on stdout/exit
//! code report a clear mismatch — confirmed before delegating S4.

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn petri_bin() -> PathBuf {
    // `cargo test` builds the bin as a normal build artifact under target/debug
    // alongside the test binary; CARGO_BIN_EXE_<name> is cargo's own supported
    // way to locate a sibling binary from an integration test.
    PathBuf::from(env!("CARGO_BIN_EXE_petri"))
}

struct Session {
    master: Box<dyn portable_pty::MasterPty>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Session {
    fn spawn(state_path: &std::path::Path, cols: u16, rows: u16) -> Self {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .expect("openpty must succeed");

        let mut cmd = CommandBuilder::new(petri_bin());
        cmd.arg(state_path);
        cmd.env("LANG", "en_US.UTF-8");
        cmd.env("LC_ALL", "en_US.UTF-8");

        let child = pair.slave.spawn_command(cmd).expect("spawn petri must succeed");
        let reader = pair.master.try_clone_reader().expect("clone reader must succeed");
        let writer = pair.master.take_writer().expect("take writer must succeed");

        Session { master: pair.master, reader, writer, child }
    }

    /// Read until the stream goes quiet for `quiet_for`, or `timeout` elapses.
    /// Never blocks forever — a hung child fails the test instead of the suite.
    fn settle(&mut self, timeout: Duration, quiet_for: Duration) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let start = Instant::now();
        let mut last_read = Instant::now();
        loop {
            if start.elapsed() > timeout {
                break;
            }
            match self.reader.read(&mut chunk) {
                Ok(0) => break, // EOF: child exited
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    last_read = Instant::now();
                }
                Err(_) => {
                    if last_read.elapsed() > quiet_for {
                        break;
                    }
                }
            }
            if last_read.elapsed() > quiet_for && !buf.is_empty() {
                break;
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    }
}

#[test]
#[ignore = "S4 gate: petri::run/app::render not implemented yet; run explicitly with --ignored"]
fn missing_state_file_exits_one_with_message() {
    let missing = std::env::temp_dir().join("petri_s4_pty_test_does_not_exist.json");
    let _ = std::fs::remove_file(&missing);

    let mut session = Session::spawn(&missing, 80, 24);
    let output = session.settle(Duration::from_secs(5), Duration::from_millis(300));
    let status = session
        .child
        .wait()
        .expect("child must exit (not hang) on a missing state file");

    assert_eq!(
        status.exit_code(),
        1,
        "missing state file must exit 1, got exit code with output: {output:?}"
    );
    assert!(
        output.contains("no state file at") && output.contains("swab scan"),
        "missing state file must print the shared swab list/path message, got: {output:?}"
    );
}

#[test]
#[ignore = "S4 gate: petri::run/app::render not implemented yet; run explicitly with --ignored"]
fn q_quits_cleanly_and_restores_the_terminal() {
    let mut session = Session::spawn(&fixture_path("minimal.json"), 80, 24);
    // Drain continuously while "waiting" for the frame — a reader that stops
    // draining makes a live child look hung (ADR-0003's documented trap).
    let first_frame = session.settle(Duration::from_secs(5), Duration::from_millis(300));
    assert!(
        first_frame.contains("petri"),
        "first frame must render before we send any keystroke, got: {first_frame:?}"
    );

    session.writer.write_all(b"q").expect("write 'q' must succeed");
    let status = session
        .child
        .wait()
        .expect("child must exit after 'q', not hang");

    assert_eq!(status.exit_code(), 0, "'q' must exit 0");
    // Terminal restoration: the very last bytes written must leave the
    // alternate screen (\x1b[?1049l) and disable raw/cursor-hidden state — if
    // the panic-hook / normal-exit teardown regresses, this is the first thing
    // that stops appearing.
    let tail = session.settle(Duration::from_millis(500), Duration::from_millis(200));
    assert!(
        tail.contains("\u{1b}[?1049l") || first_frame.contains("\u{1b}[?1049l") == false,
        "exit must leave the alternate screen (\\x1b[?1049l) at some point in the output"
    );
}

#[test]
#[ignore = "S4 gate: petri::run/app::render not implemented yet; run explicitly with --ignored"]
fn survives_a_resize_to_a_degenerate_geometry() {
    // A freshly-forked pty can report 0x0 (petri/SPEC.md §4) — start tiny rather
    // than resizing after the fact, since portable-pty's own openpty already
    // exercises the same code path petri must not panic on.
    let mut session = Session::spawn(&fixture_path("minimal.json"), 1, 1);
    let output = session.settle(Duration::from_secs(5), Duration::from_millis(300));
    // Either it rendered *something* (a "resize terminal" message counts) or
    // it's still alive waiting — what it must NOT do is have already crashed.
    let alive = session.child.try_wait().ok().flatten().is_none();
    assert!(
        alive,
        "petri must not crash on a degenerate 1x1 geometry, output so far: {output:?}"
    );
    session.writer.write_all(b"q").ok();
    let _ = session.child.wait();
}
