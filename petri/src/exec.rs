//! Handing the terminal to another program, and taking it back
//! (`petri/IDEAS.md` `MECH-2` and `MECH-3`).
//!
//! This is the primitive the whole "actions" idea rests on. `petri` is a
//! router, not a reimplementation of every tool it points at (`FRAME-2`), so
//! nearly every action it offers is some form of "run that other program" —
//! and there are exactly two ways to do that, distinguished by
//! [`crate::tools::ExecMode`].
//!
//! **Terminal ([`run_in_terminal`], `MECH-2`).** A program that draws its own
//! full-screen interface — `serie`, `lazygit`, `vim`, a pager — needs the
//! whole terminal, not a pane. So `petri` tears its own TUI down, hands the
//! child stdin/stdout/stderr wholesale, waits, and rebuilds afterwards. This
//! is what `lazygit` and `k9s` do for their own shell-outs.
//!
//! **Background ([`spawn_detached`], `MECH-3`).** A program that opens its own
//! window — `code`, `open` — must NOT take the terminal and must NOT be
//! waited on. `petri` keeps drawing; the child is detached and forgotten.
//!
//! Three things bite anyone implementing `MECH-2`, and all three are handled
//! here rather than left to the caller:
//!
//! 1. **`.status()`, never `.output()`.** `output()` captures the child's
//!    stdio into pipes, so a full-screen program renders into a buffer nobody
//!    reads and looks, from the outside, exactly like a hang.
//! 2. **`Terminal::clear()` on the way back is mandatory.** ratatui diffs each
//!    frame against a cached copy of the previous one. The child overwrote the
//!    screen without ratatui knowing, so without an explicit clear ratatui
//!    believes cells that are now garbage are still correct and only repaints
//!    the difference — producing a half-painted frame.
//! 3. **Restore on the failure path too.** If the program is missing or the
//!    spawn fails, the terminal has already been torn down. Returning early
//!    there would leave the user in a shell with no echo and no cursor.

use crate::tools::{ExecMode, Launch};
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use ratatui::Terminal;
use ratatui::backend::Backend;

/// What happened when we tried to run something.
#[derive(Debug)]
pub enum Outcome {
    /// The child ran to completion. Carries its exit status, which `petri`
    /// deliberately does not act on — a non-zero exit from `git log` because
    /// the user quit early is not `petri`'s problem to report.
    Finished(std::process::ExitStatus),
    /// The child was started and left running. Only ever returned for
    /// [`ExecMode::Background`].
    Detached,
    /// The program could not be started at all — almost always "not found",
    /// which happens when a stored tool choice went stale between the
    /// resolution and the launch.
    ///
    /// The terminal has already been fully restored by the time this is
    /// returned; the caller can render an error into the TUI as normal.
    Failed(io::Error),
}

/// Run `launch` according to its own [`ExecMode`], with `cwd` as the working
/// directory — which is how `serie` and `tig` find the repository at all, so
/// it is not optional.
///
/// The `Terminal` is borrowed mutably even in the background case so callers
/// have one entry point and cannot accidentally pick the wrong one for a
/// candidate whose mode they did not check.
pub fn run<B: Backend>(
    terminal: &mut Terminal<B>,
    launch: &Launch,
    cwd: &Path,
) -> io::Result<Outcome>
where
    io::Error: From<B::Error>,
{
    match launch.mode {
        ExecMode::Terminal => run_in_terminal(terminal, launch, cwd),
        ExecMode::Background => Ok(spawn_detached(launch, cwd)),
    }
}

/// `MECH-2`: suspend the TUI, hand the child the terminal, wait, restore.
///
/// The restore half runs on every path out of this function, including the
/// one where the child never started.
pub fn run_in_terminal<B: Backend>(
    terminal: &mut Terminal<B>,
    launch: &Launch,
    cwd: &Path,
) -> io::Result<Outcome>
where
    io::Error: From<B::Error>,
{
    suspend()?;

    // Deliberately `.status()`: the child inherits our stdin/stdout/stderr, so
    // it draws on the real terminal and reads real keystrokes.
    let result = Command::new(&launch.program)
        .args(&launch.args)
        .current_dir(cwd)
        .status();

    // Restore BEFORE inspecting the result — a failed spawn must not skip it.
    resume(terminal)?;

    Ok(match result {
        Ok(status) => Outcome::Finished(status),
        Err(e) => Outcome::Failed(e),
    })
}

/// `MECH-3`: start the child and forget it. The terminal is never touched, so
/// `petri` keeps drawing over the top.
///
/// The child's stdio is nulled rather than inherited: a GUI editor that logs
/// to stderr would otherwise scribble over the TUI at unpredictable moments.
pub fn spawn_detached(launch: &Launch, cwd: &Path) -> Outcome {
    match Command::new(&launch.program)
        .args(&launch.args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        // The child handle is dropped immediately and never waited on. On
        // Unix that leaves a zombie until `petri` itself exits, which is
        // acceptable for a process the user starts a handful of times per
        // session — and is the same trade every editor-launching TUI makes.
        Ok(_child) => Outcome::Detached,
        Err(e) => Outcome::Failed(e),
    }
}

/// Give the terminal back to the shell: leave the alternate screen and stop
/// intercepting keys.
///
/// Raw mode is disabled first, matching `ratatui::restore`'s own ordering —
/// it has the wider blast radius of the two, so it is the one to undo first.
fn suspend() -> io::Result<()> {
    crossterm::terminal::disable_raw_mode()?;
    let mut out = io::stdout();
    crossterm::execute!(out, crossterm::terminal::LeaveAlternateScreen)?;
    Ok(())
}

/// Take the terminal back and force a full repaint.
fn resume<B: Backend>(terminal: &mut Terminal<B>) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    crossterm::terminal::enable_raw_mode()?;
    let mut out = io::stdout();
    crossterm::execute!(out, crossterm::terminal::EnterAlternateScreen)?;
    // Mandatory, not defensive. See this module's doc comment, point 2: the
    // child painted over the screen without ratatui's knowledge, so ratatui's
    // cached previous frame is a lie and a diffed repaint would leave the
    // child's leftovers on screen.
    terminal.clear()?;
    Ok(())
}

/// Is `program` runnable on this machine?
///
/// This is the impure counterpart to `tools::resolve`'s injected `installed`
/// closure, and it lives here rather than in `tools` on purpose: keeping every
/// `PATH` read on this side of the boundary is what lets the whole resolution
/// layer be tested hermetically.
///
/// A name containing a path separator is checked as a path — that is how the
/// picker's "Other — specify path…" answer works when the user types
/// `/usr/local/bin/jjui` rather than a bare name. Anything else is looked up
/// across `PATH`.
pub fn is_installed(program: &str) -> bool {
    if program.is_empty() {
        return false;
    }
    if program.contains('/') {
        return is_executable(Path::new(program));
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| is_executable(&dir.join(program)))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}
