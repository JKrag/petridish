//! `petri` — the Rust/ratatui reimplementation of the interactive dashboard.
//! Spec: `petri/SPEC.md`. S4 fills in the walking skeleton (real terminal,
//! event loop, poll timer, panic hook — petri/SPEC.md §9 S4); S5+ replace
//! `app::render` with the grouped Browser/Dashboard screens.

pub mod app;
pub mod browser;
pub mod dashboard;
pub mod exec;
pub mod feed;
pub mod help;
pub mod picker;
pub mod prefs;
pub mod theme;
pub mod tools;
pub mod width;
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
    std::path::PathBuf::from(&home)
        .join(".petridish")
        .join("projects.json")
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

    // The loaded preferences are kept for the lifetime of the loop and mutated
    // in place, rather than rebuilt from scratch at each save site. Rebuilding
    // was a real bug, not a style question: every `Prefs { .. }` literal had to
    // name the new `tools` field, and each one named it as an empty map — so
    // every Tab switch silently wiped the user's stored tool choices (ACT-8's
    // whole point being that it asks once). Mutate-and-save cannot drift that
    // way when the next field is added.
    let mut prefs = prefs;
    let (mut screen, mut browser_state) = match prefs.last_screen {
        LastScreen::Dashboard => (Screen::Dashboard, None),
        LastScreen::Browser => {
            let bstate = last_good.as_ref().map(crate::browser::BrowserState::new);
            (Screen::Browser, bstate)
        }
    };
    let mut dashboard_state: Option<crate::dashboard::DashboardState> = match last_good.as_ref() {
        Some(radar) => Some(crate::dashboard::DashboardState::with_collapsed(
            radar,
            prefs.collapsed,
        )),
        None => None,
    };

    // The SPACE-1 activity feed. Deliberately NOT a field on `DashboardState`: that struct
    // is rebuilt wholesale by `DashboardState::new` on every reload below, so a feed living
    // there would be wiped on every scan tick — slice 1's finding 3 in a new costume.
    let mut feed = match last_good.as_ref() {
        Some(radar) => crate::feed::FeedState::seeded(radar),
        None => crate::feed::FeedState::default(),
    };

    // The ACT-8 tool picker, `Some` while it is open. It takes every keystroke
    // while it is up — a modal that let keys leak through to the list behind
    // it would be worse than no modal.
    let mut picker: Option<crate::picker::PickerState> = None;
    // The ACT-2 `?` help popup. Unlike the picker it has no interaction beyond
    // dismissal: any key while it is open closes it. Checked before the
    // picker/quit/screen dispatch below so it can consume every keystroke
    // while open, the same reason the picker is checked first.
    let mut help_open = false;
    // A one-line message shown over the Browser: "no tool for that", "this
    // project has no remote". Cleared by the next keystroke, so it never
    // becomes stale chrome.
    let mut notice: Option<String> = None;
    // Which action the open picker is configuring, kept alongside it so the
    // chosen program can be launched immediately rather than only stored.
    let mut picker_action: Option<crate::tools::Action> = None;

    // Initial draw — unconditional so we always paint something on startup.
    render_current(
        terminal,
        &last_good,
        screen,
        &dashboard_state,
        &browser_state,
        &picker,
        help_open,
        &notice,
        &feed,
    );

    loop {
        // crossterm's `poll` returns true when *any* event is queued (Key,
        // Resize, Mouse, FocusGained/Lost). We handle `q` to break; everything
        // else (in particular Resize) is absorbed and the next draw tick will
        // pick up the new terminal size.
        let event_ready =
            crossterm::event::poll(std::time::Duration::from_secs(1)).unwrap_or(false);
        if event_ready && let Ok(crossterm::event::Event::Key(key)) = crossterm::event::read() {
            // Any keystroke dismisses a transient notice, so it can never
            // linger as stale chrome over a screen it no longer describes.
            notice = None;

            let handled = if help_open {
                // Any key closes the popup and nothing else happens this
                // keystroke — including `q`, deliberately: accidentally
                // quitting out of a help screen would be a bad surprise.
                help_open = false;
                true
            } else if let Some(ref mut p) = picker {
                // The picker is modal: it consumes EVERY key while open,
                // including `q`. Letting `q` quit out from under an open
                // dialog would be a surprising way to lose the answer the
                // user was in the middle of giving — and `Esc` is right
                // there, advertised in the popup's own footer.
                match p.on_key(key.code) {
                    crate::picker::Outcome::Pending => {}
                    crate::picker::Outcome::Cancelled => {
                        picker = None;
                        picker_action = None;
                    }
                    crate::picker::Outcome::Chosen { program, persist } => {
                        let action = picker_action.take();
                        picker = None;
                        if let Some(action) = action {
                            // `persist` is ACT-11's verb. A one-off launch
                            // (`Enter` in re-pick mode) deliberately leaves
                            // the stored default alone — writing it here
                            // would cost the user the very default they
                            // pressed the shifted key to bypass.
                            if persist {
                                // Store first, then act. If the launch
                                // fails the user has still been asked once
                                // and only once (ACT-8).
                                prefs.tools.insert(action.id.to_string(), program.clone());
                                if let Err(e) = prefs::save(&prefs::default_prefs_path(), &prefs) {
                                    eprintln!("petri: persisting the tool choice failed: {e}");
                                }
                            }
                            notice =
                                run_action(terminal, &action, &program, &last_good, &browser_state);
                        }
                    }
                }
                true
            } else if key.code == crossterm::event::KeyCode::Char('q') {
                // `q` always quits, even in filter input mode.
                return Ok(0);
            } else if screen == Screen::Dashboard {
                // `Tab` switches Dashboard → Browser (petri/SPEC.md §5).
                if key.code == crossterm::event::KeyCode::Tab {
                    // Build browser state lazily on the first Tab switch,
                    // only if we have a valid radar (State read failures
                    // happen on first run when no state file exists yet).
                    let bstate = last_good.as_ref().map(crate::browser::BrowserState::new);
                    screen = Screen::Browser;
                    browser_state = bstate;
                    prefs.last_screen = LastScreen::Browser;
                    prefs.collapsed = dashboard_state
                        .as_ref()
                        .map(|d| d.collapsed)
                        .unwrap_or([false, false, true, true]);
                    if let Err(e) = prefs::save(&prefs::default_prefs_path(), &prefs) {
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
                            if let (Some(dstate), Some(radar)) = (&mut dashboard_state, &last_good)
                            {
                                dstate.toggle_selected(radar);
                            }
                            true
                        }
                        // `Enter`: on a header, toggle (same as Space); on a
                        // row, jump to the Browser with that project selected
                        // (petri/SPEC.md §5).
                        crossterm::event::KeyCode::Enter => {
                            if let (Some(dstate), Some(radar)) = (&mut dashboard_state, &last_good)
                            {
                                let current_row =
                                    dstate.selected.and_then(|i| dstate.visible.get(i)).copied();
                                match current_row {
                                    Some(crate::dashboard::DashRow::Header(_)) => {
                                        dstate.toggle_selected(radar);
                                    }
                                    Some(crate::dashboard::DashRow::Project(proj_idx)) => {
                                        // Persist Dashboard → Browser transition (same as Tab).
                                        prefs.last_screen = LastScreen::Browser;
                                        prefs.collapsed = dstate.collapsed;
                                        if let Err(e) =
                                            prefs::save(&prefs::default_prefs_path(), &prefs)
                                        {
                                            eprintln!(
                                                "petri S7: persist Enter→Browser failed: {e}"
                                            );
                                        }
                                        let mut bstate = crate::browser::BrowserState::new(radar);
                                        if let Some(pos) =
                                            bstate.visible.iter().position(|&i| i == proj_idx)
                                        {
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
                // subsequent character keys append to it. The flag lives on
                // `BrowserState` because `browser::render` needs it too — the
                // ACT-10 header chip draws differently while you are typing.
                if let Some(ref mut state) = browser_state {
                    state.filter_input = true;
                    state.filter_query = String::new();
                    if let Some(ref radar) = last_good {
                        state.apply_filter(radar, "");
                    }
                }
                true
            } else if browser_state.as_ref().is_some_and(|s| s.filter_input) {
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
                        if let Some(ref mut state) = browser_state {
                            state.filter_input = false;
                            state.filter_query = String::new();
                            if let Some(ref radar) = last_good {
                                state.apply_filter(radar, "");
                            }
                        }
                        true
                    }
                    // `Backspace` drops the last character of the query and
                    // re-filters. Not a "printable characters only" input:
                    // without this the only way out of a typo is `Esc` and
                    // retyping the whole query, which the ACT-10 chip made
                    // impossible to ignore once the query was on screen.
                    //
                    // `pop()` is char-wise, not byte-wise, so a multi-byte
                    // character deletes as one keypress rather than leaving
                    // a broken UTF-8 tail.
                    crossterm::event::KeyCode::Backspace => {
                        if let Some(ref mut state) = browser_state {
                            let mut q = std::mem::take(&mut state.filter_query);
                            q.pop();
                            if let Some(ref radar) = last_good {
                                state.apply_filter(radar, &q);
                            } else {
                                state.filter_query = q;
                            }
                            true
                        } else {
                            false
                        }
                    }
                    // `Enter` closes the filter input mode but keeps the
                    // query, so the filtered selection persists.
                    crossterm::event::KeyCode::Enter => {
                        if let Some(ref mut state) = browser_state {
                            state.filter_input = false;
                        }
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
                    prefs.last_screen = LastScreen::Dashboard;
                    prefs.collapsed = dashboard_state
                        .as_ref()
                        .map(|d| d.collapsed)
                        .unwrap_or([false, false, true, true]);
                    if let Err(e) = prefs::save(&prefs::default_prefs_path(), &prefs) {
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
                        // `y` (IDEAS.md ACT-2): yank the selected project's path
                        // to the clipboard. Deliberately not a tools::registry()
                        // entry — see tools.rs's module doc / IDEAS.md's ACT-2
                        // table for why. `pbcopy` is spawned directly, piped
                        // stdin, no terminal hand-off (MECH-2/MECH-3 do not
                        // apply — nothing takes over the screen).
                        crossterm::event::KeyCode::Char('y') => {
                            notice = yank_selected_path(&last_good, &browser_state);
                            true
                        }
                        // `?` (IDEAS.md ACT-2): open the help popup.
                        crossterm::event::KeyCode::Char('?') => {
                            help_open = true;
                            true
                        }
                        // `Esc` in normal mode: no-op (only meaningful to
                        // close the filter; if filter isn't open, do nothing).
                        crossterm::event::KeyCode::Esc => true,
                        // Action keys (IDEAS.md `ACT-2`). Last arm, so every
                        // navigation binding above keeps priority over the
                        // registry — a future action must never be able to
                        // silently steal `j`/`k`/`J`/`K`.
                        //
                        // Note where this sits: inside the NORMAL-mode match,
                        // never the `in_filter_input` one above. If it were in
                        // both, typing `g` into the `/` filter would launch a
                        // git browser instead of filtering. The two branches
                        // being structurally separate is what makes that safe;
                        // `s8_pty_actions.rs` gates it regardless.
                        crossterm::event::KeyCode::Char(c) => {
                            let registry = crate::tools::registry();
                            // The lowercase key runs the action; the SHIFTED
                            // variant of the same key re-picks it (ACT-11).
                            // Derived from `action.key` rather than
                            // hard-coded, so a future registry entry gets
                            // its shifted key for free. Note this sits
                            // after the J/K ×10 navigation arms, which keep
                            // priority — an action must never be able to
                            // steal a movement key.
                            let lower = registry.iter().find(|a| a.key == c).cloned();
                            let shifted = registry
                                .iter()
                                .find(|a| a.key.to_ascii_uppercase() == c && a.key != c)
                                .cloned();
                            match (lower, shifted) {
                                (Some(action), _) => {
                                    notice = begin_action(
                                        terminal,
                                        &action,
                                        &last_good,
                                        &browser_state,
                                        &prefs,
                                        &mut picker,
                                        &mut picker_action,
                                    );
                                    true
                                }
                                (None, Some(action)) => {
                                    notice = begin_repick(
                                        &action,
                                        &last_good,
                                        &browser_state,
                                        &mut picker,
                                        &mut picker_action,
                                    );
                                    true
                                }
                                (None, None) => false,
                            }
                        }
                        _ => false,
                    }
                }
            };
            if handled {
                render_current(
                    terminal,
                    &last_good,
                    screen,
                    &dashboard_state,
                    &browser_state,
                    &picker,
                    help_open,
                    &notice,
                    &feed,
                );
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
                    // The Dashboard's selection anchor has to be read here, against the
                    // OUTGOING radar, because `DashRow::Project` holds an index into
                    // `radar.projects` and `absorb_snapshot` is about to replace that list.
                    // Resolving the index afterwards would name whichever project happens to
                    // occupy that slot in the new scan — the exact silent cursor-drift the
                    // anchor exists to prevent.
                    let dash_anchor = match (&dashboard_state, &last_good) {
                        (Some(d), Some(previous)) => d.selection_anchor(previous),
                        _ => None,
                    };
                    // Feed first, by construction: `absorb_snapshot` owns both snapshots, so
                    // the previous one cannot be dropped before it has been diffed.
                    last_good = absorb_snapshot(&mut feed, last_good.take(), r);
                    // Re-derive browser state from the new Radar, preserving the
                    // current filter query. Selection follows the previously-
                    // selected project when it survives, else resets to first row
                    // (per spec §3.1 — `apply_filter` guarantees this). We take a
                    // snapshot of the filter query first so we don't hold two
                    // borrows on `browser_state` at once.
                    let query_snapshot: Option<String> =
                        browser_state.as_ref().map(|s| s.filter_query.clone());
                    if let (Some(radar), Some(q)) = (&last_good, query_snapshot)
                        && let Some(ref mut state) = browser_state
                    {
                        state.apply_filter(radar, &q);
                    }
                    // Re-derive DashboardState too, regardless of which screen
                    // is currently active, so a reload while viewing the
                    // Browser still leaves a fresh Dashboard behind it.
                    //
                    // `refresh`, not `DashboardState::new`: the latter rebuilt
                    // with the hardcoded spec defaults, so every reload reopened
                    // sections the user had collapsed and threw the cursor back
                    // to the top. On a machine `swab` is actively scanning that
                    // is every few seconds, i.e. the screen rearranging itself
                    // under the user's hands with no input from them. The
                    // `dash_anchor` was captured above, against the outgoing
                    // radar, for the reason given there.
                    if let Some(ref radar) = last_good {
                        match dashboard_state {
                            Some(ref mut d) => d.refresh(radar, dash_anchor),
                            None => {
                                dashboard_state =
                                    Some(crate::dashboard::DashboardState::with_collapsed(
                                        radar,
                                        prefs.collapsed,
                                    ))
                            }
                        }
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
            render_current(
                terminal,
                &last_good,
                screen,
                &dashboard_state,
                &browser_state,
                &picker,
                help_open,
                &notice,
                &feed,
            );
        }

        last_mtime = new_mtime;
    }
}

/// Fold a freshly-read snapshot into the activity feed and hand back the new `last_good`.
///
/// **This function exists to make the ordering unrepresentable rather than merely tested.**
/// The bug it forecloses is a one-liner: `last_good = Some(r)` destroys the previous
/// snapshot, and `FeedState::ingest` needs it — so a reload that assigns first silently
/// produces a feed that never grows a row, on a code path no unit test naturally covers.
/// Taking ownership of both halves means the caller *cannot* express that order. Same move
/// as slice 2's `run_action`, where removing a parameter beat adding a test.
///
/// - `last_good` is `None` (nothing parsed yet, first successful read): seed the feed from
///   `fresh` so a freshly-started `petri` has rows immediately.
/// - `last_good` is `Some(prev)`: ingest the `prev` -> `fresh` difference.
///
/// Returns `Some(fresh)`, which the caller stores as the new `last_good`.
pub fn absorb_snapshot(
    feed: &mut crate::feed::FeedState,
    last_good: Option<petridish_core::schema::Radar>,
    fresh: petridish_core::schema::Radar,
) -> Option<petridish_core::schema::Radar> {
    match last_good {
        // `prev` is consumed here and cannot outlive this arm, which is the point: there is
        // no way to write the replace-then-diff ordering that this function exists to
        // prevent.
        Some(prev) => feed.ingest(&prev, &fresh),
        None => *feed = crate::feed::FeedState::seeded(&fresh),
    }
    Some(fresh)
}

/// Helper: redraw `terminal` from the last good radar and whichever screen's
/// live state is currently active, with any read errors logged but not
/// propagated (mid-run failures degrade in place).
// See `render_section` in dashboard.rs: distinct render-state arguments, no
// natural grouping, so a params struct would be lint-driven noise.
#[allow(clippy::too_many_arguments)]
fn render_current(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    radar: &Option<petridish_core::schema::Radar>,
    screen: Screen,
    dashboard_state: &Option<crate::dashboard::DashboardState>,
    browser_state: &Option<crate::browser::BrowserState>,
    picker: &Option<crate::picker::PickerState>,
    help_open: bool,
    notice: &Option<String>,
    feed: &crate::feed::FeedState,
) {
    let Some(r) = radar else { return };
    match screen {
        Screen::Dashboard => {
            if let Some(s) = dashboard_state {
                let _ = terminal.draw(|frame| crate::dashboard::render(frame, r, s, feed));
            }
        }
        Screen::Browser => {
            if let Some(s) = browser_state {
                let _ = terminal.draw(|frame| {
                    crate::browser::render(frame, r, s);
                    // The overlay is drawn last, after the screen beneath it —
                    // `Clear` only blanks what is already in the buffer, so
                    // ordering is the whole mechanism (MECH-1).
                    if let Some(p) = picker {
                        crate::picker::render(frame, p);
                    } else if help_open {
                        crate::help::render(frame);
                    } else if let Some(text) = notice {
                        crate::browser::render_notice(frame, text);
                    }
                });
            }
        }
    }
}

/// The currently-selected project's path and remote URL, or `None` when
/// nothing is selected (an empty filtered list is a representable state —
/// `browser::BrowserState::selected` is deliberately an `Option`).
fn selected_project<'a>(
    radar: &'a Option<petridish_core::schema::Radar>,
    browser_state: &Option<crate::browser::BrowserState>,
) -> Option<&'a petridish_core::schema::Project> {
    let radar = radar.as_ref()?;
    let state = browser_state.as_ref()?;
    let pos = state.selected?;
    let idx = *state.visible.get(pos)?;
    radar.projects.get(idx)
}

/// Press an action key: resolve it against this machine and this project, then
/// either run it, open the picker, or explain why neither happened.
///
/// Returns the notice to display, if any. `Ok`-shaped outcomes return `None` —
/// a successful launch needs no commentary.
#[allow(clippy::too_many_arguments)]
fn begin_action(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    action: &crate::tools::Action,
    radar: &Option<petridish_core::schema::Radar>,
    browser_state: &Option<crate::browser::BrowserState>,
    prefs: &Prefs,
    picker: &mut Option<crate::picker::PickerState>,
    picker_action: &mut Option<crate::tools::Action>,
) -> Option<String> {
    let Some(project) = selected_project(radar, browser_state) else {
        return Some("nothing selected".to_string());
    };
    let facts = crate::tools::Facts {
        path: &project.path,
        url: project.git.github_url.as_deref(),
    };
    // `ACT-4`'s resolution order for the editor: the stored answer first, then
    // `$VISUAL`, then `$EDITOR`, then the registry probe. Reading the environment here
    // rather than in `tools::resolve` is what keeps that function pure and hermetically
    // testable. Measured on a real machine: both variables are frequently unset while
    // `code` sits on PATH, so this chain often yields nothing at all and the probe does
    // the real work.
    //
    // Each source is filtered by *executability*, not merely by being set — the fix for a
    // bug review caught. Taking the first source that was merely present collapsed a
    // four-step order into one guess: with `$VISUAL` naming an editor no longer on this
    // machine, `resolve` would find it uninstalled and skip straight to probing, so a
    // perfectly good `$EDITOR` was never consulted. "Present" and "usable" are different
    // questions and only the second one orders this chain.
    let usable = |name: String| crate::exec::is_installed(&name).then_some(name);
    let stored = prefs
        .tools
        .get(action.id)
        .cloned()
        .and_then(usable)
        .or_else(|| {
            (action.id == "edit")
                .then(|| {
                    std::env::var("VISUAL")
                        .ok()
                        .and_then(usable)
                        .or_else(|| std::env::var("EDITOR").ok().and_then(usable))
                })
                .flatten()
        });

    match crate::tools::resolve(action, &facts, stored.as_deref(), &|p| {
        crate::exec::is_installed(p)
    }) {
        crate::tools::Resolution::Ready(launch) => {
            launch_now(terminal, &launch, std::path::Path::new(&project.path))
        }
        crate::tools::Resolution::Ambiguous(installed) => {
            *picker = Some(crate::picker::PickerState::new(action, installed));
            *picker_action = Some(action.clone());
            None
        }
        crate::tools::Resolution::NoTool => Some(format!(
            "nothing installed that can {} — tried: {}",
            action.label,
            action
                .candidates
                .iter()
                .map(|c| c.program.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        // `ACT-9`'s per-project axis, phrased in terms of the project rather
        // than the tooling: this is the half the user can see on the row in
        // front of them.
        crate::tools::Resolution::NoTarget => Some(format!("{} has no remote", project.name)),
    }
}

/// Press a SHIFTED action key: open the re-pick popup (`ACT-11`).
///
/// Deliberately does not go through `tools::resolve`. That function collapses
/// an unambiguous choice to `Resolution::Ready` and throws the candidate list
/// away, so on the machines re-pick is *for* — one where a default already
/// resolves cleanly — there would be nothing left to list. `repick_candidates`
/// is the door that stays open.
fn begin_repick(
    action: &crate::tools::Action,
    radar: &Option<petridish_core::schema::Radar>,
    browser_state: &Option<crate::browser::BrowserState>,
    picker: &mut Option<crate::picker::PickerState>,
    picker_action: &mut Option<crate::tools::Action>,
) -> Option<String> {
    let Some(project) = selected_project(radar, browser_state) else {
        return Some("nothing selected".to_string());
    };
    let facts = crate::tools::Facts {
        path: &project.path,
        url: project.git.github_url.as_deref(),
    };
    match crate::tools::repick_candidates(action, &facts, &|p| crate::exec::is_installed(p)) {
        // `ACT-9`'s per-project axis, phrased the same way `begin_action`
        // phrases it, so the two paths never disagree on screen.
        None => Some(format!("{} has no remote", project.name)),
        // An empty list still opens the popup: `Other — specify path…` is
        // always a row, so a machine with nothing installed is still usable.
        Some(installed) => {
            *picker = Some(crate::picker::PickerState::repick(action, installed));
            *picker_action = Some(action.clone());
            None
        }
    }
}

/// Run the program the user just chose in the picker. Storing the answer
/// without acting on it would make the picker feel like a settings dialog
/// rather than the one keystroke it interrupted.
///
/// `program` is passed in explicitly rather than re-read from `prefs`, and
/// that is load-bearing for `ACT-11`. This function used to re-resolve through
/// `prefs.tools.get(action.id)`, which worked only because the caller always
/// wrote the answer to `prefs` first. A one-off launch deliberately does not
/// write it — so re-resolving would launch the user's OLD default, which is
/// exactly the behaviour the shifted key exists to escape.
///
/// The fix is structural rather than test-guarded: with no `prefs` parameter
/// in scope, this function *cannot* consult a stored answer even by accident,
/// which is a stronger guarantee than a test that a later refactor could
/// silently stop exercising. The event loop's half — persist only when the
/// picker says so — is covered by `s8_pty_repick.rs`.
fn run_action(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    action: &crate::tools::Action,
    program: &str,
    radar: &Option<petridish_core::schema::Radar>,
    browser_state: &Option<crate::browser::BrowserState>,
) -> Option<String> {
    let Some(project) = selected_project(radar, browser_state) else {
        return Some("nothing selected".to_string());
    };
    let facts = crate::tools::Facts {
        path: &project.path,
        url: project.git.github_url.as_deref(),
    };
    let launch = crate::tools::launch_for(action, &facts, program);
    launch_now(terminal, &launch, std::path::Path::new(&project.path))
}

/// Copy the selected project's path to the system clipboard via a piped
/// `pbcopy` child. macOS-only tool (matches the rest of petridish), so on the
/// Linux leg of the CI matrix this always degrades to a notice rather than
/// panicking or silently doing nothing — see invariant 5's "sensors degrade,
/// never abort" ethos, applied here even though this isn't a sensor.
fn yank_selected_path(
    radar: &Option<petridish_core::schema::Radar>,
    browser_state: &Option<crate::browser::BrowserState>,
) -> Option<String> {
    let Some(project) = selected_project(radar, browser_state) else {
        return Some("nothing selected".to_string());
    };
    let path = project.path.clone();
    let mut child = match std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Some(format!("could not copy to clipboard: {e}")),
    };
    let write_result = child.stdin.take().map(|mut stdin| {
        use std::io::Write;
        stdin.write_all(path.as_bytes())
    });
    if let Some(Err(e)) = write_result {
        let _ = child.wait();
        return Some(format!("could not copy to clipboard: {e}"));
    }
    match child.wait() {
        Ok(status) if status.success() => Some(format!("copied {path} to clipboard")),
        Ok(status) => Some(format!(
            "could not copy to clipboard: pbcopy exited {status}"
        )),
        Err(e) => Some(format!("could not copy to clipboard: {e}")),
    }
}

/// Run one resolved launch, turning every failure into a notice rather than an
/// error that would take the TUI down. A tool that is missing at launch time
/// (uninstalled since it was chosen) is a message, not a crash.
fn launch_now(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    launch: &crate::tools::Launch,
    cwd: &std::path::Path,
) -> Option<String> {
    match crate::exec::run(terminal, launch, cwd) {
        Ok(crate::exec::Outcome::Finished(_)) | Ok(crate::exec::Outcome::Detached) => None,
        Ok(crate::exec::Outcome::Failed(e)) => {
            Some(format!("could not run {}: {e}", launch.program))
        }
        Err(e) => Some(format!("terminal hand-off failed: {e}")),
    }
}
