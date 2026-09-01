//! `petri` — the Rust/ratatui reimplementation of the interactive dashboard.
//! Spec: `petri/SPEC.md`. S4 fills in the walking skeleton (real terminal,
//! event loop, poll timer, panic hook — petri/SPEC.md §9 S4); S5+ replace
//! `app::render` with the grouped Browser/Dashboard screens.

pub mod app;
pub mod browser;
pub mod dashboard;
pub mod prefs;
pub mod theme;
use crate::prefs::{LastScreen, Prefs};

/// Row count for the Browser's `Shift`-style fast-jump keys (`J`/`K`). 10 is
/// the fixed "about ten lines" jump — deliberately NOT tied to viewport
/// height (unlike `PageUp`/`PageDown`, which jump exactly one screenful):
/// this is the small/predictable hop, that one is the big/screen-relative one.
const BROWSER_FAST_JUMP: i32 = 10;

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

    // Step 1.5: the initial state read and prefs load ALSO happen before any
    // terminal mutation, for the same reason as the existence check above —
    // any eprintln! warning either one produces (a corrupt state file on
    // first read, a missing/corrupt petri.toml) must land on a normal,
    // non-alternate-screen stderr. Doing this after EnterAlternateScreen was
    // a real bug, not a theoretical one: on the very first run (no
    // petri.toml yet, the expected shape for every new user) the "missing
    // preferences file" warning collided with the header's first draw and
    // visibly corrupted it — caught by smoke-testing against real data.
    let initial_radar = match read_state_file(state_path) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("petri: initial state read failed: {e}");
            None
        }
    };
    let prefs = prefs::load(&prefs::default_prefs_path());

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
    let exit_code = poll_loop(state_path, &mut terminal, initial_radar, prefs)?;

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


/// Which screen `poll_loop` is currently rendering. Dashboard is the default
/// landing screen (petri/SPEC.md §3.2 frames it as the ambient monitor) — S6
/// wires a one-way `Enter`-on-a-row transition to the Browser; `Tab` to
/// switch back is S7's job (petri/SPEC.md §9), not implemented here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Dashboard,
    Browser,
}

/// The main poll loop: draw the current state once, then only redraw on
/// meaningful events (keyboard input, resize, mtime change). `q` breaks.
///
/// `last_good` and `prefs` are read by the caller (`run`) BEFORE the
/// alternate screen is entered, not here — see `run`'s Step 1.5 doc comment
/// for why a warning from either must never fire once the alt screen is
/// live.
fn poll_loop(
    state_path: &std::path::Path,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    mut last_good: Option<petridish_core::schema::Radar>,
    prefs: Prefs,
) -> std::io::Result<u8> {
    // Initial mtime snapshot. We don't draw on ticks where nothing has
    // changed — the initial draw below is unconditional so we always paint
    // something on startup, but subsequent mtime comparisons rely on this
    // baseline. Mutated on each iteration so the new value propagates forward.
    let mut last_mtime = std::fs::metadata(state_path)
        .ok()
        .and_then(|m| m.modified().ok());

    let (mut screen, mut browser_state) = match prefs.last_screen {
        LastScreen::Dashboard => {
            (Screen::Dashboard, None)
        }
        LastScreen::Browser => {
            let bstate = last_good.as_ref().map(crate::browser::BrowserState::new);
            (Screen::Browser, bstate)
        }
    };
    let mut dashboard_state: Option<crate::dashboard::DashboardState> = match last_good.as_ref() {
        Some(radar) => Some(crate::dashboard::DashboardState::with_collapsed(radar, prefs.collapsed)),
        None => None,
    };

    let mut in_filter_input = false;

    // Initial draw — unconditional so we always paint something on startup.
    render_current(terminal, &last_good, screen, &dashboard_state, &browser_state);

    loop {
        // crossterm's `poll` returns true when *any* event is queued (Key,
        // Resize, Mouse, FocusGained/Lost). We handle `q` to break; everything
        // else (in particular Resize) is absorbed and the next draw tick will
        // pick up the new terminal size.
        let event_ready = crossterm::event::poll(std::time::Duration::from_secs(1)).unwrap_or(false);
        if event_ready {
            if let Ok(crossterm::event::Event::Key(key)) = crossterm::event::read() {
                let handled = if key.code == crossterm::event::KeyCode::Char('q') {
                    // `q` always quits, even in filter input mode.
                    return Ok(0);
                } else if screen == Screen::Dashboard {
                    // `Tab` switches Dashboard → Browser (petri/SPEC.md §5).
                    if key.code == crossterm::event::KeyCode::Tab {
                        // Build browser state lazily on the first Tab switch,
                        // only if we have a valid radar (State read failures
                        // happen on first run when no state file exists yet).
                        let bstate = last_good
                            .as_ref()
                            .map(crate::browser::BrowserState::new);
                        screen = Screen::Browser;
                        browser_state = bstate;
                        if let Err(e) = prefs::save(&prefs::default_prefs_path(), &Prefs {
                            last_screen: LastScreen::Browser,
                            collapsed: dashboard_state.as_ref().map(|d| d.collapsed).unwrap_or([false, false, true, true]),
                            tools: std::collections::BTreeMap::new(),
                        }) {
                            eprintln!("petri S7: persist Tab switch failed: {e}");
                        }
                        true
                    } else {
                        match key.code {
                            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                                if let Some(ref mut dstate) = dashboard_state {
                                    dstate.move_selection(-1);
                                }
                                true
                            }
                            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                                if let Some(ref mut dstate) = dashboard_state {
                                    dstate.move_selection(1);
                                }
                                true
                            }
                            crossterm::event::KeyCode::Char(' ') => {
                                if let (Some(dstate), Some(radar)) = (&mut dashboard_state, &last_good) {
                                    dstate.toggle_selected(radar);
                                }
                                true
                            }
                            // `Enter`: on a header, toggle (same as Space); on a
                            // row, jump to the Browser with that project selected
                            // (petri/SPEC.md §5).
                            crossterm::event::KeyCode::Enter => {
                                if let (Some(dstate), Some(radar)) = (&mut dashboard_state, &last_good) {
                                    let current_row = dstate.selected.and_then(|i| dstate.visible.get(i)).copied();
                                    match current_row {
                                        Some(crate::dashboard::DashRow::Header(_)) => {
                                            dstate.toggle_selected(radar);
                                        }
                                        Some(crate::dashboard::DashRow::Project(proj_idx)) => {
                                            // Persist Dashboard → Browser transition (same as Tab).
                                            if let Err(e) = prefs::save(&prefs::default_prefs_path(), &Prefs {
                                                last_screen: LastScreen::Browser,
                                                collapsed: dstate.collapsed,
                                                tools: std::collections::BTreeMap::new(),
                                            }) {
                                                eprintln!("petri S7: persist Enter→Browser failed: {e}");
                                            }
                                            let mut bstate = crate::browser::BrowserState::new(radar);
                                            if let Some(pos) = bstate.visible.iter().position(|&i| i == proj_idx) {
                                                bstate.selected = Some(pos);
                                            }
                                            browser_state = Some(bstate);
                                            screen = Screen::Browser;
                                        }
                                        None => {}
                                    }
                                }
                                true
                            }
                            crossterm::event::KeyCode::Esc => true,
                            _ => false,
                        }
                    }
                } else if key.code == crossterm::event::KeyCode::Char('/') {
                    // Enter filter input mode. The query starts empty and
                    // subsequent character keys append to it.
                    in_filter_input = true;
                    if let Some(ref mut state) = browser_state {
                        state.filter_query = String::new();
                        if let Some(ref radar) = last_good {
                            state.apply_filter(radar, "");
                        }
                    }
                    true
                } else if in_filter_input {
                    match key.code {
                        // Navigation arrows and j/k still move selection while
                        // in filter mode (the user may want to test moves without
                        // exiting the filter). Place before the generic Char(c)
                        // arm so they take priority.
                        crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                            if let Some(ref mut state) = browser_state {
                                state.move_selection(-1);
                            }
                            true
                        }
                        crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                            if let Some(ref mut state) = browser_state {
                                state.move_selection(1);
                            }
                            true
                        }
                        // Page/fast-jump/edge navigation, same as normal mode
                        // (see that match arm's comments) — none of these are
                        // printable characters that a filter query could want,
                        // so binding them here doesn't cost the user anything
                        // they could otherwise type.
                        crossterm::event::KeyCode::PageUp => {
                            if let Some(ref mut state) = browser_state {
                                let step = crossterm::terminal::size()
                                    .map(|(_, h)| crate::browser::page_size(h) as i32)
                                    .unwrap_or(BROWSER_FAST_JUMP);
                                state.move_selection(-step);
                            }
                            true
                        }
                        crossterm::event::KeyCode::PageDown => {
                            if let Some(ref mut state) = browser_state {
                                let step = crossterm::terminal::size()
                                    .map(|(_, h)| crate::browser::page_size(h) as i32)
                                    .unwrap_or(BROWSER_FAST_JUMP);
                                state.move_selection(step);
                            }
                            true
                        }
                        crossterm::event::KeyCode::Home => {
                            if let Some(ref mut state) = browser_state {
                                state.move_selection(i32::MIN);
                            }
                            true
                        }
                        crossterm::event::KeyCode::End => {
                            if let Some(ref mut state) = browser_state {
                                state.move_selection(i32::MAX);
                            }
                            true
                        }
                        // `Esc` closes the filter input mode *and* clears the
                        // query (petri/SPEC.md §5).
                        crossterm::event::KeyCode::Esc => {
                            in_filter_input = false;
                            if let Some(ref mut state) = browser_state {
                                state.filter_query = String::new();
                                if let Some(ref radar) = last_good {
                                    state.apply_filter(radar, "");
                                }
                            }
                            true
                        }
                        // `Enter` closes the filter input mode but keeps the
                        // query, so the filtered selection persists.
                        crossterm::event::KeyCode::Enter => {
                            in_filter_input = false;
                            true
                        }
                        // Character keys: append to the query (filter input
                        // only — we don't treat these as navigation when we're
                        // mid-filter). Non-printable / control keys fall
                        // through and are ignored in filter mode.
                        crossterm::event::KeyCode::Char(c) => {
                            if let Some(ref mut state) = browser_state {
                                let q = std::mem::take(&mut state.filter_query);
                                let new_q = format!("{q}{c}");
                                if let Some(ref radar) = last_good {
                                    state.apply_filter(radar, &new_q);
                                }
                                true
                            } else {
                                false
                            }
                        }
                        _ => false,
                    }
                } else {
                    // `Tab` from the Browser switches back to the Dashboard
                    // (petri/SPEC.md §5). Persistence is handled in the
                    // Dashboard branch above, but here on the Browser side
                    // it must also trigger a save (the Dashboard branch
                    // doesn't fire when screen is Browser).
                    if key.code == crossterm::event::KeyCode::Tab {
                        if let Err(e) = prefs::save(&prefs::default_prefs_path(), &Prefs {
                            last_screen: LastScreen::Dashboard,
                            collapsed: dashboard_state.as_ref().map(|d| d.collapsed).unwrap_or([false, false, true, true]),
                            tools: std::collections::BTreeMap::new(),
                        }) {
                            eprintln!("petri S7: persist Tab switch (Browser→Dashboard) failed: {e}");
                        }
                        screen = Screen::Dashboard;
                        true
                    } else {
                        match key.code {
                            // Navigation in normal (non-filter) mode.
                            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                                if let Some(ref mut state) = browser_state {
                                    state.move_selection(-1);
                                }
                                true
                            }
                            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                                if let Some(ref mut state) = browser_state {
                                    state.move_selection(1);
                                }
                                true
                            }
                            // Fast jump: ~10 rows, a fixed hop independent of
                            // viewport size (PageUp/PageDown below is the
                            // screen-relative jump).
                            crossterm::event::KeyCode::Char('K') => {
                                if let Some(ref mut state) = browser_state {
                                    state.move_selection(-BROWSER_FAST_JUMP);
                                }
                                true
                            }
                            crossterm::event::KeyCode::Char('J') => {
                                if let Some(ref mut state) = browser_state {
                                    state.move_selection(BROWSER_FAST_JUMP);
                                }
                                true
                            }
                            // Page jump: exactly one screenful, matching the
                            // list's own real visible-row count (`browser::page_size`
                            // mirrors `browser::render`'s layout math). Falls back to
                            // the fixed fast-jump distance if the terminal size can't
                            // be read.
                            crossterm::event::KeyCode::PageUp => {
                                if let Some(ref mut state) = browser_state {
                                    let step = crossterm::terminal::size()
                                        .map(|(_, h)| crate::browser::page_size(h) as i32)
                                        .unwrap_or(BROWSER_FAST_JUMP);
                                    state.move_selection(-step);
                                }
                                true
                            }
                            crossterm::event::KeyCode::PageDown => {
                                if let Some(ref mut state) = browser_state {
                                    let step = crossterm::terminal::size()
                                        .map(|(_, h)| crate::browser::page_size(h) as i32)
                                        .unwrap_or(BROWSER_FAST_JUMP);
                                    state.move_selection(step);
                                }
                                true
                            }
                            // Jump straight to the first/last row.
                            crossterm::event::KeyCode::Home => {
                                if let Some(ref mut state) = browser_state {
                                    state.move_selection(i32::MIN);
                                }
                                true
                            }
                            crossterm::event::KeyCode::End => {
                                if let Some(ref mut state) = browser_state {
                                    state.move_selection(i32::MAX);
                                }
                                true
                            }
                            // `Esc` in normal mode: no-op (only meaningful to
                            // close the filter; if filter isn't open, do nothing).
                            crossterm::event::KeyCode::Esc => true,
                            _ => false,
                        }
                    }
                };
                if handled {
                    render_current(terminal, &last_good, screen, &dashboard_state, &browser_state);
                }
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
                Ok(r) => {
                    last_good = Some(r);
                    // Re-derive browser state from the new Radar, preserving the
                    // current filter query. Selection follows the previously-
                    // selected project when it survives, else resets to first row
                    // (per spec §3.1 — `apply_filter` guarantees this). We take a
                    // snapshot of the filter query first so we don't hold two
                    // borrows on `browser_state` at once.
                    let query_snapshot: Option<String> = browser_state
                        .as_ref()
                        .map(|s| s.filter_query.clone());
                    if let (Some(radar), Some(q)) = (&last_good, query_snapshot) {
                        if let Some(ref mut state) = browser_state {
                            state.apply_filter(radar, &q);
                        }
                    }
                    // Re-derive DashboardState too, regardless of which screen
                    // is currently active, so a reload while viewing the
                    // Browser still leaves a fresh Dashboard behind it. Collapse
                    // state resets to spec defaults on reload rather than being
                    // preserved — not gated by any acceptance test, and `rebuild`
                    // is private to `dashboard.rs`, so this is the simplest
                    // correct behavior rather than a deliberate UX call.
                    if let Some(ref radar) = last_good {
                        dashboard_state = Some(crate::dashboard::DashboardState::new(radar));
                    }
                }
                Err(e) => eprintln!("petri S5 mid-loop state read failed: {e}"),
            }
        }

        // Redraw only when something actually happened this tick: a crossterm
        // event (resize gets picked up here) or an mtime change. On quiet
        // ticks we skip draw so the output stream goes still — this keeps PTY
        // harnesses happy and the user's terminal clean when petri is idle.
        if event_ready || mtime_changed {
            render_current(terminal, &last_good, screen, &dashboard_state, &browser_state);
        }

        last_mtime = new_mtime;
    }
}

/// Helper: redraw `terminal` from the last good radar and whichever screen's
/// live state is currently active, with any read errors logged but not
/// propagated (mid-run failures degrade in place).
fn render_current(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    radar: &Option<petridish_core::schema::Radar>,
    screen: Screen,
    dashboard_state: &Option<crate::dashboard::DashboardState>,
    browser_state: &Option<crate::browser::BrowserState>,
) {
    let Some(r) = radar else { return };
    match screen {
        Screen::Dashboard => {
            if let Some(s) = dashboard_state {
                let _ = terminal.draw(|frame| crate::dashboard::render(frame, r, s));
            }
        }
        Screen::Browser => {
            if let Some(s) = browser_state {
                let _ = terminal.draw(|frame| crate::browser::render(frame, r, s));
            }
        }
    }
}

