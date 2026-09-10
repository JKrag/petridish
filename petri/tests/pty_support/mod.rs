//! Shared PTY test harness (petri/SPEC.md §8 layer 3, ADR-0003), protected,
//! authored by the orchestrator. Used via `mod pty_support;` from each PTY
//! integration test file (Rust's standard `tests/<name>/mod.rs` convention for
//! sharing code between integration-test binaries without it being treated as
//! its own test binary).
//!
//! `dead_code` is allowed for the whole module, and that is not blanket
//! silencing: `mod pty_support;` compiles a *separate copy* of this file into
//! every PTY test binary, and each of those uses a different subset of the
//! harness. `wait_with_timeout` has nine callers and `screen_retry` seven, yet
//! each is "never used" from the point of view of the binaries that happen not
//! to call it. Without this, a green `-D warnings` build would demand deleting
//! helpers that are demonstrably in use.
//!
//! `spawn_and_settle_nonempty` and its `_with_home` variant used to live here and are
//! deliberately GONE rather than deprecated. Their contract was "re-spawn while the output
//! is empty", and that contract is itself the bug: petri's first write need not be a frame
//! — S7's "preferences file missing, using defaults" warning goes to stderr before the
//! alternate screen is entered — so the output goes non-empty, the retry loop declares
//! success, and the caller asserts against a warning line. Every test that used them was
//! flaky for exactly that reason. Use `screen_until` and name the frame you are waiting
//! for; there is no case left where "any bytes at all" is the right condition.
#![allow(dead_code)]

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

/// The Dashboard's header badge — `dashboard.rs`'s `HEADER_TITLE`, verbatim.
///
/// This is the marker for "petri has taken the terminal and painted a frame", and the
/// reason it is a header badge rather than the obvious `"petri"` is the same trap this
/// module's doc comment describes for `spawn_and_settle_nonempty`: `prefs::load` warns
/// `petri S7: preferences file ... missing or unreadable` on a scratch home, and lib.rs's
/// "Step 1.5" emits it deliberately BEFORE `enable_raw_mode` so it cannot corrupt the first
/// draw. So `"petri"` is on screen while the terminal is still in canonical mode, where a
/// keystroke is buffered by the line discipline and then discarded when raw mode comes on.
/// A badge cannot appear until after the alternate-screen entry that follows raw mode, so
/// waiting for one is waiting for the thing that actually makes a keystroke deliverable.
pub const DASHBOARD_HEADER: &str = " petri \u{b7} dashboard ";

/// The Browser's header badge — `browser.rs`'s title span, verbatim.
///
/// The marker for "a `Tab` was actually processed". `"browser"` is not: the *Dashboard's*
/// footer reads `Enter open/browser` (`dashboard.rs`'s `footer_line`), so a wait on that
/// needle is already satisfied by the pre-Tab frame and returns without the Tab having been
/// handled at all — which then hides a lost keystroke behind a passing wait.
pub const BROWSER_HEADER: &str = " petri \u{b7} browser ";

pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join(name)
}

pub fn petri_bin() -> PathBuf {
    // `cargo test` builds the bin as a normal build artifact under target/debug
    // alongside the test binary; CARGO_BIN_EXE_<name> is cargo's own supported
    // way to locate a sibling binary from an integration test.
    PathBuf::from(env!("CARGO_BIN_EXE_petri"))
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
        Self::spawn_inner(state_path, cols, rows, None, &[])
    }

    /// Like `spawn`, but overrides `HOME` for the child process — needed to
    /// point `petri`'s prefs-file resolution (`~/.petridish/petri.toml`,
    /// petri/SPEC.md §6) at a scratch directory, since unlike the state path
    /// that path is never a CLI arg. Each call gets its own child process
    /// (and therefore its own env), so this is safe under `--test-threads=1`
    /// without the shared-`HOME`-mutation hazard CLAUDE.md documents for
    /// same-process fixture tests.
    pub fn spawn_with_home(
        state_path: &std::path::Path,
        cols: u16,
        rows: u16,
        home: &std::path::Path,
    ) -> Self {
        Self::spawn_inner(state_path, cols, rows, Some(home), &[])
    }

    /// Like `spawn_with_home`, but appends `extra_args` after the state-path
    /// positional — the only way to reach `petri --mini` (issue #31), whose
    /// whole contract is about how a flag and that positional interact
    /// (`petri::parse_args`). `home` is `Option` here rather than two more
    /// wrappers because a `--mini` test needs both axes independently.
    ///
    /// Additive: every existing entry point below delegates to this with an
    /// empty slice, so no already-written PTY test changes behaviour.
    pub fn spawn_with_args(
        state_path: &std::path::Path,
        cols: u16,
        rows: u16,
        home: Option<&std::path::Path>,
        extra_args: &[&str],
    ) -> Self {
        Self::spawn_inner(state_path, cols, rows, home, extra_args)
    }

    fn spawn_inner(
        state_path: &std::path::Path,
        cols: u16,
        rows: u16,
        home: Option<&std::path::Path>,
        extra_args: &[&str],
    ) -> Self {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty must succeed");

        // Reader + drain thread set up BEFORE spawning the child — see this
        // module's doc comment, bug 2.
        let mut reader = pair
            .master
            .try_clone_reader()
            .expect("clone reader must succeed");
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
        for arg in extra_args {
            cmd.arg(arg);
        }
        cmd.env("LANG", "en_US.UTF-8");
        cmd.env("LC_ALL", "en_US.UTF-8");
        if let Some(home) = home {
            cmd.env("HOME", home);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .expect("spawn petri must succeed");
        // Drop the slave end in this process once spawned — otherwise our own
        // held fd keeps the pty "open" from the reader's perspective and EOF
        // (Ok(0)) never arrives after the child actually exits.
        drop(pair.slave);

        Session {
            _master: pair.master,
            writer,
            child,
            chunks: rx,
            accumulated: Vec::new(),
        }
    }

    /// Pull everything the drain thread has produced so far into `accumulated`,
    /// blocking up to `timeout` total, and returning once nothing new has
    /// arrived for `quiet_for`. Never blocks longer than `timeout` even if the
    /// child is still alive and producing output.
    ///
    /// `quiet_for` is a *trailing* quiet period: it only applies once at least
    /// one byte has arrived. Before then the only bound is `timeout`.
    ///
    /// That distinction is the whole point, and getting it wrong made
    /// `s10_pty_reload` fail roughly half the time under `cargo test
    /// --workspace` while passing every time under `cargo test -p petri`.
    /// Applying `quiet_for` to a still-empty buffer conflates "the child has
    /// finished painting" with "the child has not started yet", so a caller
    /// asking for `settle(5s, 300ms)` actually got a 300ms budget for first
    /// output — and `screen_retry`'s five attempts made that a 1.5s budget in
    /// total, not the 5s the call site plainly reads as. In a workspace run
    /// petri's first spawn follows swab's ~45s suite, and a cold 2.3MB debug
    /// binary does not reliably paint that fast. The failure signature was an
    /// all-blank grid at a near-constant 1.52-1.53s, which is that arithmetic
    /// rather than anything the test was asserting about.
    pub fn settle(&mut self, timeout: Duration, quiet_for: Duration) -> String {
        let start = Instant::now();
        loop {
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                break;
            }
            // Wait the full remaining budget while we have nothing at all; once
            // bytes are flowing, fall back to the trailing quiet window.
            let wait = if self.accumulated.is_empty() {
                remaining
            } else {
                remaining.min(quiet_for)
            };
            match self.chunks.recv_timeout(wait) {
                Ok(bytes) => self.accumulated.extend_from_slice(&bytes),
                Err(mpsc::RecvTimeoutError::Timeout) => break, // quiet window elapsed with nothing new
                Err(mpsc::RecvTimeoutError::Disconnected) => break, // drain thread exited (EOF)
            }
        }
        String::from_utf8_lossy(&self.accumulated).to_string()
    }

    /// Reconstruct the current on-screen `cols`×`rows` grid from everything
    /// drained so far (settling first), by feeding the accumulated bytes
    /// through a real `vt100::Parser` (built on `vte`, Alacritty's parser)
    /// rather than replaying only the handful of sequences a hand-rolled
    /// parser recognised.
    ///
    /// This exists because petri/SPEC.md §8 names raw byte-stream substring
    /// matching as the root cause of the Python TUI's worst CI flakiness —
    /// "the captured failing CI frame was [...] the whole RUNNING section
    /// simply absent, i.e. a partially-painted frame. Reading the pty byte
    /// stream also means 'lines' are stream segments, not screen rows,
    /// which is what broke the other assertion." Asserting against a
    /// reconstructed screen makes a partial/interleaved write show up as
    /// wrong content in a specific cell, not a coincidentally-still-passing
    /// substring check.
    ///
    /// This module used to hand-roll its own CSI parsing — absolute cursor
    /// positioning, erase-in-display/-line, and `CSI ? 1049 h` only, with
    /// everything else (SGR, scroll regions, line wrapping, tab stops,
    /// relative cursor movement) silently dropped rather than flagged. A
    /// spike compared the two implementations' reconstructed grids for the
    /// same captured byte stream row by row: 0 of 30 rows differed, so the
    /// hand-rolled version was correct for what ratatui's crossterm backend
    /// happens to emit today — but a future ratatui version reaching for any
    /// of the dropped sequences would silently produce a wrong grid no test
    /// could detect. `vt100` closes that exposure (issue #47).
    ///
    /// `Cell::contents()` is `""`, not `" "`, for a cell nothing has written
    /// to — mapped to a space here so unwritten padding still reads as
    /// blank rather than collapsing every row's trailing whitespace.
    /// Assumes one terminal cell per `char` (no wide-glyph/combining-
    /// character handling), same as before; acceptable because no PTY test
    /// asserts against a fixture with CJK/emoji names (`hostile.json`'s
    /// exist only for JSON-parsing coverage in other layers).
    pub fn screen(
        &mut self,
        cols: u16,
        rows: u16,
        timeout: Duration,
        quiet_for: Duration,
    ) -> Vec<String> {
        let raw = self.settle(timeout, quiet_for);
        let mut parser = vt100::Parser::new(rows, cols, 0);
        parser.process(raw.as_bytes());
        let screen = parser.screen();

        (0..rows)
            .map(|r| {
                (0..cols)
                    .map(|c| {
                        let contents = screen.cell(r, c).map(|cell| cell.contents()).unwrap_or("");
                        if contents.is_empty() { " " } else { contents }
                    })
                    .collect::<String>()
            })
            .collect()
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
    pub fn screen_retry(
        &mut self,
        cols: u16,
        rows: u16,
        timeout: Duration,
        quiet_for: Duration,
        attempts: u32,
    ) -> Vec<String> {
        let mut grid = self.screen(cols, rows, timeout, quiet_for);
        let mut attempt = 1;
        while grid.iter().all(|line| line.trim().is_empty()) && attempt < attempts {
            attempt += 1;
            eprintln!(
                "screen_retry attempt {attempt}/{attempts}: blank grid (suspected PTY race), retrying"
            );
            grid = self.screen(cols, rows, timeout, quiet_for);
        }
        grid
    }

    /// Like `screen_retry`, but retries against a caller-supplied predicate
    /// over the reconstructed grid instead of a fixed "is it blank" check.
    ///
    /// `screen_retry` only catches the empty-output race (nothing painted
    /// yet). It does NOT catch a different race this module hadn't needed
    /// until a post-keystroke assertion needed it: after writing a key that
    /// should trigger a redraw (e.g. `Space` opening a popup), the settle
    /// window can elapse and return a grid *before* the child has actually
    /// processed the key and repainted — which produces a perfectly
    /// well-formed, non-blank grid (the PREVIOUS frame), indistinguishable
    /// from a real content mismatch to a single-shot check or to
    /// `screen_retry`'s blank-only retry. Confirmed as the actual failure
    /// mode via CI (petri/tests/s5_pty.rs's popup-toggle test failed
    /// deterministically on both `macos-14` and `ubuntu-latest` runners with
    /// a fully-painted but stale pre-keystroke frame, not a blank one, while
    /// passing locally every time — consistent with CI's timing making this
    /// narrower race land more often than on a lightly-loaded dev machine).
    ///
    /// Retries until `predicate` returns true or `attempts` is exhausted,
    /// returning the LAST grid either way — same "never silently pass a
    /// genuine regression" contract as `screen_retry` and
    /// `spawn_and_settle_nonempty`.
    pub fn screen_until(
        &mut self,
        cols: u16,
        rows: u16,
        timeout: Duration,
        quiet_for: Duration,
        attempts: u32,
        mut predicate: impl FnMut(&[String]) -> bool,
    ) -> Vec<String> {
        let mut grid = self.screen(cols, rows, timeout, quiet_for);
        let mut attempt = 1;
        while !predicate(&grid) && attempt < attempts {
            attempt += 1;
            eprintln!(
                "screen_until attempt {attempt}/{attempts}: predicate not yet satisfied, retrying"
            );
            grid = self.screen(cols, rows, timeout, quiet_for);
        }
        grid
    }

    /// Settle repeatedly until `predicate` accepts the RAW accumulated stream, or
    /// `attempts` is exhausted. Returns whether the predicate was ever satisfied.
    ///
    /// The grid-based `screen_until` is the right tool for "has the screen reached this
    /// state". This one exists for the case it cannot express: **an event whose completion
    /// leaves the screen looking exactly as it did before.** A terminal hand-off is that
    /// case — petri leaves the alternate screen, runs the child, re-enters and repaints the
    /// same content — so no predicate over the reconstructed grid can distinguish "the
    /// hand-off finished" from "the hand-off has not started". The control sequences can:
    /// they carry the transition the grid throws away.
    ///
    /// Deliberately NOT for content assertions. `SPEC.md` §8 names raw byte-stream
    /// substring matching as the root cause of the Python TUI's worst CI flakiness, and
    /// that verdict stands — a *frame* must be asserted against the grid. This is for
    /// waiting on a terminal-mode transition, which is not content at all.
    pub fn settle_until_raw(
        &mut self,
        timeout: Duration,
        quiet_for: Duration,
        attempts: u32,
        mut predicate: impl FnMut(&str) -> bool,
    ) -> bool {
        for attempt in 1..=attempts {
            let stream = self.settle(timeout, quiet_for);
            if predicate(&stream) {
                return true;
            }
            if attempt < attempts {
                eprintln!(
                    "settle_until_raw attempt {attempt}/{attempts}: not satisfied yet, retrying"
                );
            }
        }
        false
    }

    /// How many times the child has entered the alternate screen so far.
    ///
    /// One at startup; a second one only after a terminal hand-off has run its child and
    /// come back. That makes `>= 2` the precise, non-timing-based answer to "is petri in
    /// charge of the terminal again", which is what a test must know before it can send
    /// another keystroke — a key written while the child still owns the terminal is queued
    /// by the line discipline in canonical mode and lost when raw mode is restored.
    ///
    /// Counts transitions of `vt100::Screen::alternate_screen()` from false to true, rather
    /// than substring-matching `\x1b[?1049h` in the raw stream directly (the pre-#47 version
    /// of this function) — the transition still has to be tracked by hand, since vt100
    /// exposes only the *current* mode, not a count of times it was entered, but it is now
    /// vt100's own real escape-sequence state machine doing the recognition rather than a
    /// literal byte-pattern search.
    ///
    /// The scratch parser's geometry is a generous fixed 200×200, not the caller's real
    /// session size: this call site never asks about grid content, only mode state, but
    /// `vt100`'s internal scroll/cursor-clamp math still runs against whatever dimensions
    /// it's given — a too-small grid (tried 1×1 first, since content is irrelevant) panics
    /// with `attempt to subtract with overflow` in `vt100`'s own `grid.rs` once cursor
    /// movement in a real session's stream (this crashed against s4_pty's degenerate 1×1
    /// *session* geometry test, at 80×24 real content) references coordinates the parser's
    /// grid is too small to represent.
    pub fn alt_screen_entries(stream: &str) -> usize {
        let mut parser = vt100::Parser::new(200, 200, 0);
        let mut was_alt = false;
        let mut entries = 0;
        for byte in stream.as_bytes() {
            parser.process(std::slice::from_ref(byte));
            let now_alt = parser.screen().alternate_screen();
            if now_alt && !was_alt {
                entries += 1;
            }
            was_alt = now_alt;
        }
        entries
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
