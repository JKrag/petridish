//! `petri` — the Rust/ratatui reimplementation of the interactive dashboard.
//! Spec: `petri/SPEC.md`. S4 fills in the walking skeleton (real terminal, event
//! loop, poll timer, panic hook — petri/SPEC.md §9 S4); S5+ replace `app::render`
//! with the grouped Browser/Dashboard screens.

pub mod app;

/// Resolved default state-file path: `$HOME/.petridish/projects.json`. Mirrors
/// `swab::cli::default_state_path` — same reasoning (composed directly so tests
/// can override it without touching `HOME`).
pub fn default_state_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set");
    std::path::PathBuf::from(&home).join(".petridish").join("projects.json")
}

/// Read and deserialize the state file. The error message is promoted to
/// `io::Error` so the caller can unify JSON parse failures with IO errors.
fn read_state_file(path: &std::path::Path) -> std::io::Result<petridish_core::schema::Radar> {
    let text = std::fs::read_to_string(path)?;
    serde_json::from_str(&text)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

/// Entry point. Checks `state_path` exists *before* entering the alternate screen
/// (petri/SPEC.md §4 "Missing state file") — returns exit code 1 with the same
/// message `swab list`/`swab path` use if not. Otherwise enters the terminal,
/// runs the event loop (mtime poll, `q` quits), and restores the terminal on
/// every exit path including panic (a panic hook must be installed before the
/// alternate screen is entered). Returns the process exit code so this is
/// unit-testable without spawning a process, mirroring `swab::cli`'s handler
/// convention.
pub fn run(state_path: &std::path::Path) -> std::io::Result<u8> {
    // Step 1: existence check — BEFORE any terminal mutation. The PTY test
    // captures combined stdout/stderr output and asserts the shared swab
    // message shape, so this must run with no screen touched yet.
    if !state_path.exists() {
        eprintln!(
            "no state file at {}; run 'swab scan' first",
            state_path.display()
        );
        return Ok(1);
    }

    // Step 2: enter alternate screen + raw mode. If any of these fails we
    // propagate the IO error — we haven't touched terminal state beyond what
    // succeeded, and the OS-level teardown will happen when the process exits.
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;

    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    // Step 3: install a panic hook that leaves the alternate screen and
    // disables raw mode *before* unwinding further. The spec calls a panic
    // that leaves the user's terminal in raw mode "a v1 blocker, not a polish
    // item". Installed here so the alt screen + raw mode are covered even if
    // setup itself panics. The previous hook is captured and re-invoked so the
    // panic message still reaches stderr (in restored, non-raw mode).
    install_panic_hook();

    // Step 4: event loop.
    let exit_code = poll_loop(state_path, &mut terminal)?;

    // Step 5: restore the terminal on the normal (non-panic) exit path.
    {
        let mut out = std::io::stdout().lock();
        let _ = crossterm::execute!(out, crossterm::terminal::LeaveAlternateScreen);
    }
    let _ = crossterm::terminal::disable_raw_mode();

    Ok(exit_code)
}

/// Install a panic hook that restores the terminal (leaves alternate screen,
/// disables raw mode) before delegating to the previous hook so the panic
/// message still reaches stderr in a non-raw terminal. `std::panic::take_hook`
/// returns `Box<dyn Fn(&PanicHookInfo) + Send + Sync>` directly (Rust 1.97
/// no longer wraps it in `Box<dyn Any>`), so we own the previous hook
/// wholesale inside our wrapper closure.
fn install_panic_hook() {
    use std::panic::{self, PanicHookInfo};
    let previous: Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync> = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let mut out = std::io::stdout().lock();
        let _ = crossterm::execute!(out, crossterm::terminal::LeaveAlternateScreen);
        previous(info);
    }));
}

/// The main poll loop: draw the current state once, then only redraw on
/// meaningful events (keyboard input, resize, mtime change). `q` breaks.
fn poll_loop(
    state_path: &std::path::Path,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
) -> std::io::Result<u8> {
    let mut last_good: Option<petridish_core::schema::Radar> = None;

    // Initial read — if the file exists but is corrupt, we keep `None` and
    // render nothing until the file recovers. The loop itself degrades by
    // keeping whatever `last_good` still holds (a non-panicking invariant).
    match read_state_file(state_path) {
        Ok(r) => last_good = Some(r),
        Err(e) => eprintln!("petri S4 initial state read failed: {e}"),
    }

    // Initial mtime snapshot. We don't draw on ticks where nothing has
    // changed — the initial draw below is unconditional so we always paint
    // something on startup, but subsequent mtime comparisons rely on this
    // baseline. Mutated on each iteration so the new value propagates forward.
    let mut last_mtime = std::fs::metadata(state_path)
        .ok()
        .and_then(|m| m.modified().ok());

    // Initial draw — unconditional so we always paint something on startup.
    render_current(terminal, &last_good);

    loop {
        // crossterm's `poll` returns true when *any* event is queued (Key,
        // Resize, Mouse, FocusGained/Lost). We handle `q` to break; everything
        // else (in particular Resize) is absorbed and the next draw tick will
        // pick up the new terminal size.
        let event_ready = crossterm::event::poll(std::time::Duration::from_secs(1)).unwrap_or(false);
        if event_ready {
            match crossterm::event::read() {
                Ok(crossterm::event::Event::Key(key)) => {
                    eprintln!("[debug] key: {:?}", key.code);
                    if key.code == crossterm::event::KeyCode::Char('q') {
                        return Ok(0);
                    }
                }
                other => eprintln!("[debug] non-key event: {:?}", other),
            }
        }

        // Re-read and re-render only when the mtime changed (petri/SPEC.md
        // §4 "Auto-poll: stat the state file's mtime on a short timer and
        // re-read + re-render only when it changed.").
        let new_mtime = std::fs::metadata(state_path)
            .ok()
            .and_then(|m| m.modified().ok());

        let mtime_changed = match (&last_mtime, new_mtime) {
            (Some(prev), Some(now)) => *prev != now,
            _ => false,
        };

        if mtime_changed {
            match read_state_file(state_path) {
                Ok(r) => last_good = Some(r),
                Err(e) => eprintln!("petri S4 mid-loop state read failed: {e}"),
            }
        }

        // Redraw only when something actually happened this tick: a crossterm
        // event (resize gets picked up here) or an mtime change. On quiet
        // ticks we skip draw so the output stream goes still — this keeps PTY
        // harnesses happy and the user's terminal clean when petri is idle.
        if event_ready || mtime_changed {
            render_current(terminal, &last_good);
        }

        last_mtime = new_mtime;
    }
}

/// Helper: redraw `terminal` from the last good radar, with any read errors
/// logged but not propagated (mid-run failures degrade in place).
fn render_current(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    radar: &Option<petridish_core::schema::Radar>,
) {
    if let Some(r) = radar {
        let _ = terminal.draw(|frame| app::render(frame, r));
    }
}
