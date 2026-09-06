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
//! 2. **Invalidating ratatui's cached frame on the way back is mandatory.**
//!    ratatui diffs each frame against a cached copy of the previous one. The
//!    child overwrote the screen without ratatui knowing, so without an
//!    explicit clear ratatui believes cells that are now garbage are still
//!    correct and only repaints the difference — producing a half-painted
//!    frame. Note it must NOT be done with `Terminal::clear()`, for a reason
//!    `resume` below documents in full.
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
        // Hand the child to a thread that does nothing but wait on it. Dropping
        // the handle instead would leave a zombie on Unix until `petri` itself
        // exits: `Child`'s Drop is explicitly documented not to reap. An earlier
        // version of this comment called that acceptable "for a process the user
        // starts a handful of times per session", which mis-frames the lifetime —
        // `petri` is an ambient monitor meant to stay open for days, and the
        // commonest target here is macOS `open`, which exits at once and so leaves
        // a zombie every single time. The thread costs one blocked `waitpid` and
        // ends when the child does.
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Outcome::Detached
        }
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
    crossterm::terminal::enable_raw_mode()
        .map_err(|e| io::Error::other(format!("enable_raw_mode: {e}")))?;
    let mut out = io::stdout();
    crossterm::execute!(out, crossterm::terminal::EnterAlternateScreen)
        .map_err(|e| io::Error::other(format!("EnterAlternateScreen: {e}")))?;
    // Invalidate ratatui's cached previous frame. Mandatory, not defensive:
    // the child painted over the screen without ratatui's knowledge, so that
    // cache is a lie and a diffed repaint would leave the child's leftovers
    // on screen.
    //
    // `Terminal::resize` rather than the more obvious `Terminal::clear`, and
    // the difference is not cosmetic. In ratatui 0.30 `clear()` snapshots the
    // cursor first — `backend.get_cursor_position()` writes a DSR query and
    // blocks waiting for the terminal to answer. That is a synchronous
    // round-trip to the emulator at the single worst moment, immediately after
    // taking the screen back, and it fails outright wherever something else is
    // draining the tty: under our own PTY harness it times out with "the
    // cursor position could not be read", which is how this was found.
    //
    // `resize` on a fullscreen viewport reaches the same `clear_viewport` —
    // a full `ClearType::All` plus a back-buffer reset — with no query at all.
    // Re-reading the size on the way through is a bonus rather than a cost:
    // the window may genuinely have been resized while the child owned it, and
    // `size()` is an ioctl, not a terminal round-trip.
    let area = terminal
        .size()
        .map_err(|e| io::Error::other(format!("Terminal::size: {}", io::Error::from(e))))?;
    terminal
        .resize(area.into())
        .map_err(|e| io::Error::other(format!("Terminal::resize: {}", io::Error::from(e))))?;
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

/// Like `is_installed`, but understands the `"app:<Name>"` probe key
/// `Candidate::as_app` produces: checks `/Applications/<Name>.app` exists
/// instead of a `PATH` lookup. Any other string is passed straight through
/// to `is_installed`.
pub fn is_installed_probe(probe: &str) -> bool {
    match probe.strip_prefix("app:") {
        Some(app_name) => Path::new(&format!("/Applications/{app_name}.app")).exists(),
        None => is_installed(probe),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_installed_probe_checks_the_applications_folder_for_an_app_probe() {
        assert!(!is_installed_probe(
            "app:Definitely Not A Real App That Exists"
        ));
    }

    #[test]
    fn is_installed_probe_behaves_like_is_installed_for_a_plain_program() {
        assert_eq!(is_installed_probe("git"), is_installed("git"));
        assert_eq!(
            is_installed_probe("definitely-not-a-real-binary-xyz"),
            is_installed("definitely-not-a-real-binary-xyz")
        );
    }
}
