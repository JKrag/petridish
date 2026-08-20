//! Shared PTY test harness (petri/SPEC.md §8 layer 3, ADR-0003), protected,
//! authored by the orchestrator. Used via `mod pty_support;` from each PTY
//! integration test file (Rust's standard `tests/<name>/mod.rs` convention for
//! sharing code between integration-test binaries without it being treated as
//! its own test binary).
//!
//! Extracted from S4's `s4_pty.rs` once S5 needed the identical harness — see
//! that module's git history for the two real bugs found and fixed here:
//! 1. A dedicated background thread drains the pty for the ENTIRE session
//!    lifetime, not just during discrete "wait for a frame" calls, because
//!    `Child::wait()` blocks — if petri writes another frame (e.g. a poll
//!    tick) while nobody is reading, the pty buffer fills, petri blocks in
//!    `write()`, and `wait()` never returns. This is the exact trap ADR-0003
//!    documents ("stop draining the pty and the child blocks in write(),
//!    which looks exactly like the TUI ignoring your keystroke").
//! 2. `pair.master` is kept alive for the whole session (`_master` field) and
//!    the reader thread is started BEFORE the child is spawned. Both close an
//!    intermittent (~1/15-1/30 runs) empty-output race on petri's fastest
//!    exit paths, where the master pty handle being dropped (or the reader
//!    thread not yet scheduled) raced the child's write-then-exit.

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("fixtures")
        .join(name)
}

pub fn petri_bin() -> PathBuf {
    // `cargo test` builds the bin as a normal build artifact under target/debug
    // alongside the test binary; CARGO_BIN_EXE_<name> is cargo's own supported
    // way to locate a sibling binary from an integration test.
    PathBuf::from(env!("CARGO_BIN_EXE_petri"))
}

/// Repeatedly (re)spawn+settle up to `attempts` times, returning the first
/// non-empty settled output (and the `Session` that produced it, still
/// alive, for further interaction). Mitigates the empty-first-settle PTY
/// race documented in this module's doc comment (bug 2's fix narrows it but
/// doesn't fully close it) — S4's and S5's PTY tests each independently
/// reinvented this same bounded-retry loop before it was pulled up here for
/// S6 to reuse. Returns the LAST (possibly still empty) output if every
/// attempt comes back empty, so a genuine regression fails loudly rather
/// than silently passing.
pub fn spawn_and_settle_nonempty(
    state_path: &std::path::Path,
    cols: u16,
    rows: u16,
    timeout: Duration,
    quiet_for: Duration,
    attempts: u32,
) -> (Session, String) {
    let mut session = Session::spawn(state_path, cols, rows);
    let mut output = session.settle(timeout, quiet_for);
    let mut attempt = 1;
    while output.is_empty() && attempt < attempts {
        attempt += 1;
        eprintln!("attempt {attempt}/{attempts}: empty output (suspected PTY race), retrying");
        session = Session::spawn(state_path, cols, rows);
        output = session.settle(timeout, quiet_for);
    }
    (session, output)
}

pub struct Session {
    // Kept alive for the whole session even though nothing reads it directly —
    // see this module's doc comment, bug 2.
    _master: Box<dyn portable_pty::MasterPty>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
    chunks: mpsc::Receiver<Vec<u8>>,
    accumulated: Vec<u8>,
}

impl Session {
    pub fn spawn(state_path: &std::path::Path, cols: u16, rows: u16) -> Self {
        Self::spawn_inner(state_path, cols, rows, None)
    }

    /// Like `spawn`, but overrides `HOME` for the child process — needed to
    /// point `petri`'s prefs-file resolution (`~/.petridish/petri.toml`,
    /// petri/SPEC.md §6) at a scratch directory, since unlike the state path
    /// that path is never a CLI arg. Each call gets its own child process
    /// (and therefore its own env), so this is safe under `--test-threads=1`
    /// without the shared-`HOME`-mutation hazard CLAUDE.md documents for
    /// same-process fixture tests.
    pub fn spawn_with_home(state_path: &std::path::Path, cols: u16, rows: u16, home: &std::path::Path) -> Self {
        Self::spawn_inner(state_path, cols, rows, Some(home))
    }

    fn spawn_inner(state_path: &std::path::Path, cols: u16, rows: u16, home: Option<&std::path::Path>) -> Self {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .expect("openpty must succeed");

        // Reader + drain thread set up BEFORE spawning the child — see this
        // module's doc comment, bug 2.
        let mut reader = pair.master.try_clone_reader().expect("clone reader must succeed");
        let writer = pair.master.take_writer().expect("take writer must succeed");

        let (tx, rx) = mpsc::channel::<Vec<u8>>();
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
        if let Some(home) = home {
            cmd.env("HOME", home);
        }

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
    pub fn settle(&mut self, timeout: Duration, quiet_for: Duration) -> String {
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

    /// Reconstruct the current on-screen `cols`×`rows` grid from everything
    /// drained so far (settling first), by replaying the ANSI/VT sequences
    /// ratatui's crossterm backend actually emits: absolute cursor
    /// positioning (`CSI row;col H`), erase-in-display/-line (`CSI n J` /
    /// `CSI n K`), SGR color/style (`CSI ... m`, ignored — no grid effect),
    /// and `CSI ? 1049 h` (alt-screen entry, clears the grid).
    ///
    /// This exists because petri/SPEC.md §8 names raw byte-stream substring
    /// matching as the root cause of the Python TUI's worst CI flakiness —
    /// "the captured failing CI frame was [...] the whole RUNNING section
    /// simply absent, i.e. a partially-painted frame. Reading the pty byte
    /// stream also means 'lines' are stream segments, not screen rows,
    /// which is what broke the other assertion." Asserting against a
    /// reconstructed screen makes a partial/interleaved write show up as
    /// wrong content in a specific cell, not a coincidentally-still-passing
    /// substring check (S6's `s6_pty.rs` hit exactly this substring-matching
    /// trap once, fixed ad hoc there — this is the systematic fix S7 needed
    /// per spec anyway, since S7's Tab-switch test is the first one where a
    /// diff-redraw genuinely interleaves two screens' content).
    ///
    /// Deliberately NOT a full VT100 emulator — only what crossterm's
    /// backend actually emits, per the sequences enumerated above. Assumes
    /// one terminal cell per `char` (no wide-glyph/combining-character
    /// handling); acceptable because no PTY test asserts against a fixture
    /// with CJK/emoji names (`hostile.json`'s exist only for JSON-parsing
    /// coverage in other layers).
    pub fn screen(&mut self, cols: u16, rows: u16, timeout: Duration, quiet_for: Duration) -> Vec<String> {
        let raw = self.settle(timeout, quiet_for);
        let (cols, rows) = (cols as usize, rows as usize);
        let mut grid: Vec<Vec<char>> = vec![vec![' '; cols]; rows];
        let (mut row, mut col): (usize, usize) = (0, 0);

        let chars: Vec<char> = raw.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c == '\u{1b}' && i + 1 < chars.len() && chars[i + 1] == '[' {
                let start_params = i + 2;
                let mut j = start_params;
                while j < chars.len() && !chars[j].is_ascii_alphabetic() {
                    j += 1;
                }
                if j >= chars.len() {
                    break; // truncated sequence at the end of a settle window
                }
                let final_byte = chars[j];
                let raw_params: String = chars[start_params..j].iter().collect();
                let is_private = raw_params.starts_with('?');
                let params_str = raw_params.trim_start_matches('?');
                let params: Vec<i64> = params_str.split(';').filter_map(|s| s.parse::<i64>().ok()).collect();

                match final_byte {
                    'H' | 'f' => {
                        let r = params.first().copied().unwrap_or(1).max(1) as usize;
                        let c2 = params.get(1).copied().unwrap_or(1).max(1) as usize;
                        row = (r - 1).min(rows.saturating_sub(1));
                        col = (c2 - 1).min(cols.saturating_sub(1));
                    }
                    'J' => {
                        let mode = params.first().copied().unwrap_or(0);
                        if mode == 2 || mode == 3 {
                            for line in grid.iter_mut() {
                                line.iter_mut().for_each(|cell| *cell = ' ');
                            }
                        }
                    }
                    'K' if row < rows => {
                        let mode = params.first().copied().unwrap_or(0);
                        match mode {
                            1 => (0..=col.min(cols.saturating_sub(1))).for_each(|cc| grid[row][cc] = ' '),
                            2 => (0..cols).for_each(|cc| grid[row][cc] = ' '),
                            _ => (col..cols).for_each(|cc| grid[row][cc] = ' '),
                        }
                    }
                    'h' if is_private && params_str == "1049" => {
                        for line in grid.iter_mut() {
                            line.iter_mut().for_each(|cell| *cell = ' ');
                        }
                    }
                    _ => {} // SGR, mode toggles, cursor show/hide — no grid effect.
                }
                i = j + 1;
                continue;
            }
            match c {
                '\n' => row = (row + 1).min(rows.saturating_sub(1)),
                '\r' => col = 0,
                '\u{7}' => {}
                _ => {
                    if row < rows && col < cols {
                        grid[row][col] = c;
                    }
                    col += 1;
                }
            }
            i += 1;
        }

        grid.into_iter().map(|line| line.into_iter().collect()).collect()
    }

    /// Like `screen`, but retries the settle+parse step (NOT a respawn — the
    /// same session, same accumulated buffer) up to `attempts` times if the
    /// reconstructed grid comes back entirely blank. `Session::accumulated`
    /// is cumulative and never cleared between calls, so once the delayed
    /// bytes actually arrive on the channel, a later call picks them up
    /// automatically — this is the same empty-output race
    /// `spawn_and_settle_nonempty` mitigates for the raw-string case,
    /// measured here at a much higher rate for a single un-retried `screen`
    /// call (4/5 blank in one local run) than the ~1/15-1/30 documented for
    /// `settle` alone, likely because `screen`'s shorter default settle
    /// window per call narrows the race window further. Returns the LAST
    /// (possibly still blank) grid if every attempt comes back blank, so a
    /// genuine regression still fails loudly.
    pub fn screen_retry(&mut self, cols: u16, rows: u16, timeout: Duration, quiet_for: Duration, attempts: u32) -> Vec<String> {
        let mut grid = self.screen(cols, rows, timeout, quiet_for);
        let mut attempt = 1;
        while grid.iter().all(|line| line.trim().is_empty()) && attempt < attempts {
            attempt += 1;
            eprintln!("screen_retry attempt {attempt}/{attempts}: blank grid (suspected PTY race), retrying");
            grid = self.screen(cols, rows, timeout, quiet_for);
        }
        grid
    }

    /// Wait for the child to exit, with a hard timeout so a genuine hang fails
    /// this test instead of the whole suite. The drain thread keeps running
    /// throughout (it owns its own reader clone), so this cannot deadlock the
    /// way a bare `child.wait()` could if nothing were draining concurrently.
    pub fn wait_with_timeout(&mut self, timeout: Duration) -> portable_pty::ExitStatus {
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
