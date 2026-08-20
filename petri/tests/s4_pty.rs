//! S4 acceptance gate, layer 3 (petri/SPEC.md §8, ADR-0003) — protected, authored
//! by the orchestrator, not the delegate. Drives the real compiled `petri` binary
//! in a PTY. Deterministic by construction (ADR-0003): a dedicated background
//! thread drains the pty continuously for the entire session lifetime — not just
//! during discrete "wait for a frame" calls — because `child.wait()` blocks, and
//! if petri writes another frame (e.g. a poll tick) while nobody is reading, the
//! pty buffer fills, petri blocks in `write()`, and `wait()` never returns. This
//! is the exact trap ADR-0003 documents ("stop draining the pty and the child
//! blocks in write(), which looks exactly like the TUI ignoring your keystroke")
//! — the first version of this file reproduced it by only draining inside
//! `settle()` and calling `child.wait()` outside that window. Sets the winsize
//! explicitly (a forked pty starts at 0x0). Reads until the stream goes quiet
//! before asserting rather than pattern-matching the raw byte stream. No
//! fixed-clock injection needed yet — S4 renders no time-derived text.
//!
//! Was `#[ignore]`d while `petri::run`/`app::render` were `todo!()` stubs
//! (all three tests failed on that basis, confirmed before delegating S4);
//! stripped now that S4 landed and all three pass, including 4 consecutive
//! re-runs to rule out the exact flakiness ADR-0003 warns this layer is prone
//! to.

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
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
    // Kept alive for the whole session even though nothing reads it directly —
    // dropping the master pty handle can tear down the underlying pty device
    // while the cloned reader (owned by the drain thread) is still using it.
    // This was the actual cause of an intermittent empty-output flake: `pair
    // .master` used to be a bare local in `spawn`, dropped as soon as this
    // function returned, racing the drain thread's startup.
    _master: Box<dyn portable_pty::MasterPty>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Every chunk the drain thread reads, in order, as it arrives.
    chunks: mpsc::Receiver<Vec<u8>>,
    /// Everything received so far via `chunks`, accumulated by `settle`.
    accumulated: Vec<u8>,
}

impl Session {
    fn spawn(state_path: &std::path::Path, cols: u16, rows: u16) -> Self {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .expect("openpty must succeed");

        // Set up the reader + drain thread BEFORE spawning the child. A very
        // fast-exiting child (e.g. petri's missing-state-file path: one
        // eprintln! then exit) can write and close its end of the pty within
        // microseconds — clone-reader-then-spawn-thread AFTER spawning the
        // child raced that exit often enough to flake (~1/15 runs) even with
        // `pair.master` kept alive. Spawning the child last, once the reader
        // thread is already blocked in its first `read()` syscall, removes
        // the race entirely: the fork/exec cost alone (hundreds of
        // microseconds) is ample scheduling headroom.
        let mut reader = pair.master.try_clone_reader().expect("clone reader must succeed");
        let writer = pair.master.take_writer().expect("take writer must succeed");

        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        // Dedicated drain thread: reads for the ENTIRE session lifetime, so the
        // child is never blocked on write() no matter what the test is doing
        // (including sitting inside a blocking child.wait()) — the deadlock
        // described in the module doc comment above.
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break, // EOF: pty closed
                    Ok(n) => {
                        if tx.send(chunk[..n].to_vec()).is_err() {
                            break; // receiver dropped (test over)
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let mut cmd = CommandBuilder::new(petri_bin());
        cmd.arg(state_path);
        cmd.env("LANG", "en_US.UTF-8");
        cmd.env("LC_ALL", "en_US.UTF-8");

        let child = pair.slave.spawn_command(cmd).expect("spawn petri must succeed");
        // Drop the slave end in this process once spawned — otherwise our own
        // held fd keeps the pty "open" from the reader's perspective and EOF
        // (Ok(0)) never arrives after the child actually exits.
        drop(pair.slave);

        Session { _master: pair.master, writer, child, chunks: rx, accumulated: Vec::new() }
    }

    /// Pull everything the drain thread has produced so far into `accumulated`,
    /// blocking up to `timeout` total, and returning once nothing new has
    /// arrived for `quiet_for`. Never blocks longer than `timeout` even if the
    /// child is still alive and producing output.
    fn settle(&mut self, timeout: Duration, quiet_for: Duration) -> String {
        let start = Instant::now();
        loop {
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                break;
            }
            match self.chunks.recv_timeout(remaining.min(quiet_for)) {
                Ok(bytes) => self.accumulated.extend_from_slice(&bytes),
                Err(mpsc::RecvTimeoutError::Timeout) => break, // quiet_for elapsed with nothing new
                Err(mpsc::RecvTimeoutError::Disconnected) => break, // drain thread exited (EOF)
            }
        }
        String::from_utf8_lossy(&self.accumulated).to_string()
    }

    /// Wait for the child to exit, with a hard timeout so a genuine hang fails
    /// this test instead of the whole suite. The drain thread keeps running
    /// throughout (it owns its own reader clone), so this cannot deadlock the
    /// way a bare `child.wait()` could if nothing were draining concurrently.
    fn wait_with_timeout(&mut self, timeout: Duration) -> portable_pty::ExitStatus {
        // portable_pty::Child isn't Clone/Send-friendly enough to hand to a
        // waiter thread here, so poll try_wait from this thread instead.
        let start = Instant::now();
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return status;
            }
            if start.elapsed() > timeout {
                panic!("child did not exit within {timeout:?} — this is a hang, not a slow pass");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[test]
fn missing_state_file_exits_one_with_message() {
    // Retry wrapper for a real, narrow OS-level PTY race: this is petri's
    // FASTEST exit path (one eprintln! then exit(1), no terminal setup at
    // all), and on macOS an already-buffered write can occasionally be lost
    // if the slave side closes fast enough after writing — observed at
    // roughly 1/15-1/30 runs even with the master pty kept alive and the
    // reader thread started before the child spawns (see Session::spawn's
    // comment for the mitigations already in place). ADR-0003 is explicit
    // that a flaky layer-3 test is worse than none in an unattended context,
    // so rather than accept a nonzero flake rate, retry the whole
    // spawn+settle cycle up to 3 times and only fail if it's consistently
    // empty — which would mean a real regression, not this race.
    let missing = std::env::temp_dir().join("petri_s4_pty_test_does_not_exist.json");
    let _ = std::fs::remove_file(&missing);

    let mut output = String::new();
    let mut status = None;
    for attempt in 1..=3 {
        let mut session = Session::spawn(&missing, 80, 24);
        output = session.settle(Duration::from_secs(5), Duration::from_millis(300));
        status = Some(session.wait_with_timeout(Duration::from_secs(5)));
        if !output.is_empty() {
            break;
        }
        eprintln!("attempt {attempt}/3: empty output (suspected PTY race), retrying");
    }
    let status = status.expect("at least one attempt must have run");

    assert_eq!(
        status.exit_code(),
        1,
        "missing state file must exit 1, got exit code with output: {output:?}"
    );
    assert!(
        output.contains("no state file at") && output.contains("swab scan"),
        "missing state file must print the shared swab list/path message after 3 attempts, got: {output:?}"
    );
}

#[test]
fn q_quits_cleanly_and_restores_the_terminal() {
    let mut session = Session::spawn(&fixture_path("minimal.json"), 80, 24);
    let first_frame = session.settle(Duration::from_secs(5), Duration::from_millis(300));
    assert!(
        first_frame.contains("petri"),
        "first frame must render before we send any keystroke, got: {first_frame:?}"
    );

    session.writer.write_all(b"q").expect("write 'q' must succeed");
    let status = session.wait_with_timeout(Duration::from_secs(5));

    assert_eq!(status.exit_code(), 0, "'q' must exit 0");
    // Terminal restoration: the accumulated output must contain the leave-
    // alternate-screen sequence somewhere — if the panic-hook / normal-exit
    // teardown regresses, this is the first thing that stops appearing.
    let tail = session.settle(Duration::from_millis(500), Duration::from_millis(200));
    let whole = format!("{first_frame}{tail}");
    assert!(
        whole.contains("\u{1b}[?1049l"),
        "exit must leave the alternate screen (\\x1b[?1049l) at some point in the output, got: {whole:?}"
    );
}

#[test]
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
    let _ = session.wait_with_timeout(Duration::from_secs(5));
}
