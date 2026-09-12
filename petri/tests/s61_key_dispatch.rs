//! Issue #61: `handle_key`'s Dashboard/Browser dispatch, proven against
//! `ratatui::backend::TestBackend` instead of a real terminal.
//!
//! These are three of the ~16 PTY tests the #48 audit flagged as conceptually
//! pure-state assertions ("key dispatch/state-transition assertions...
//! wearing rendered content as their only observable") blocked only by
//! `poll_loop`/`mini_poll_loop` being hardcoded to
//! `Terminal<CrosstermBackend<Stdout>>` — migrated here now that `handle_key`
//! is generic over `Backend` (issue #61's own resolution). Each PTY original
//! is removed from its file, with that file's module doc comment updated to
//! point here — the same convention #59 used for its own two removals.
//!
//! Migrated:
//! - `s6_pty.rs`'s `enter_on_a_row_switches_from_dashboard_to_browser`
//! - `s13_pty_dashboard_actions.rs`'s
//!   `an_action_on_the_dashboard_reaches_the_selected_project`
//! - `s13_pty_dashboard_actions.rs`'s
//!   `an_action_still_fires_while_the_focus_popup_is_open`
//! - `s8_pty_help.rs`'s `help_popup_opens_and_closes_on_any_key`
//! - `s11_pty_focus.rs`'s five state/rendering tests (`space_on_a_project_row_opens_the_focus_popup`,
//!   `esc_closes_the_focus_popup`, `space_twice_on_a_row_closes_what_it_opened`,
//!   `space_on_a_header_still_collapses_the_section`,
//!   `a_small_terminal_renders_the_panel_full_screen_instead_of_a_popup`) — that file's
//!   own module doc comment called itself "lifecycle and wiring" checks added only because
//!   there was no other way to drive `press_space`/`close_focus` before #61; its sixth test,
//!   `q_still_exits_zero_after_the_focus_gesture`, is a real lifecycle check (raw
//!   mode/terminal restore on exit) and stays PTY.
//! - `s8_pty_actions.rs`'s two tests (`action_keys_do_not_fire_while_the_filter_has_focus`,
//!   `an_action_on_a_project_with_no_remote_reports_it`) — that file is now empty, deleted.
//!
//! What stays PTY (not migrated, and not attempted here): anything that
//! actually launches a program (MECH-2/MECH-3) or asserts a real process's
//! exit code — `handle_key` still has to prove those through a real
//! terminal, which is exactly why `begin_action`/`launch_now` are the one
//! part of dispatch this file never calls with anything but a
//! `NoTarget`-resolving action (`o` on a no-remote project): a notice, never
//! a launch, so there is no side effect to fake or avoid.
//!
//! **`handle_key` takes an explicit `prefs_path` for exactly this file's benefit.**
//! The `Enter`-switches-screen test below persists the screen switch (same
//! as `Tab`), and the very first version of this test called `handle_key`
//! with no way to redirect that write — it went to this machine's real
//! `~/.petridish/petri.toml` and overwrote it, because `prefs::default_prefs_path()`
//! reads the real `$HOME`, and a PTY test's usual fix (a scratch `$HOME` env
//! var for a spawned subprocess) does nothing for a function called
//! in-process. Every test here builds its own scratch path instead — see
//! `scratch_prefs_path` below — never the real one.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use petri::dashboard::DashboardState;
use petri::prefs::Prefs;
use petri::{KeyOutcome, Screen, handle_key, render_current};
use petridish_core::schema::{AgentState, GitState, Project, Radar, SCHEMA_VERSION, StatusBucket};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

/// A prefs-file path under a per-test scratch directory, never the real
/// `~/.petridish/petri.toml` — see this module's doc comment for why that
/// distinction is load-bearing here specifically.
fn scratch_prefs_path(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("petri_s61_dispatch_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir must be creatable");
    dir.join("petri.toml")
}

fn project(id: &str, name: &str) -> Project {
    Project {
        id: id.to_string(),
        name: name.to_string(),
        path: format!("/repos/{id}"),
        category: "default".to_string(),
        parent_path: None,
        is_foreign: false,
        git: GitState::not_a_repo(),
        agent: AgentState::idle_unknown(),
        last_activity_at: None,
        status_bucket: StatusBucket::Active,
        agent_activity: Vec::new(),
    }
}

fn radar_of(projects: Vec<Project>) -> Radar {
    Radar {
        schema_version: SCHEMA_VERSION,
        updated_at: chrono::Utc::now(),
        scan_duration_ms: 0,
        projects,
        quota: None,
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn rendered_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let area = buffer.area();
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replaces `s6_pty.rs`'s `enter_on_a_row_switches_from_dashboard_to_browser`.
///
/// Same two keystrokes as the PTY original (`j` to move off the section
/// header onto the project row, then `Enter`), same property: `Enter` on a
/// Dashboard row jumps to the Browser (petri/SPEC.md §3.2/§5). The PTY
/// version could only prove this by waiting on the Browser's "Projects"
/// marker to appear in a real terminal frame; here the transition is the
/// `Screen` value itself, no waiting or terminal required.
#[test]
fn enter_on_a_dashboard_row_switches_to_browser() {
    let radar = radar_of(vec![project("alpha", "alpha")]);
    let mut screen = Screen::Dashboard;
    let mut dashboard_state = Some(DashboardState::with_collapsed(
        &radar,
        Prefs::default().collapsed,
    ));
    let mut browser_state: Option<petri::browser::BrowserState> = None;
    let mut picker = None;
    let mut picker_action = None;
    let mut help_open = false;
    let mut notice = None;
    let mut prefs = Prefs::default();
    let prefs_path = scratch_prefs_path("enter_switch");
    let last_good = Some(radar);
    let mut terminal =
        Terminal::new(TestBackend::new(80, 24)).expect("TestBackend terminal must construct");

    // `DashboardState::rebuild`'s first stop is always a section header, so
    // one `j` moves the cursor onto the project row before `Enter` targets it
    // — the same single deterministic step the PTY original took.
    let after_j = handle_key(
        key(KeyCode::Char('j')),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );
    assert_eq!(after_j, KeyOutcome::Continue(true));
    assert_eq!(
        screen,
        Screen::Dashboard,
        "'j' alone must not switch screens"
    );

    let after_enter = handle_key(
        key(KeyCode::Enter),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );
    assert_eq!(after_enter, KeyOutcome::Continue(true));
    assert_eq!(
        screen,
        Screen::Browser,
        "Enter on a Dashboard row must jump to the Browser (petri/SPEC.md §3.2/§5)"
    );
    assert!(
        browser_state.is_some(),
        "the Browser's state must be built on the transition, not left None"
    );
}

/// Replaces `s13_pty_dashboard_actions.rs`'s
/// `an_action_on_the_dashboard_reaches_the_selected_project`.
///
/// `o` (open remote) on a project with no `github_url` resolves
/// `Resolution::NoTarget` — a notice, never a launch (issue #64), so this has
/// no side effect on the machine running the test, same reason the PTY
/// original picked it. Asserts the notice is actually drawn over the
/// Dashboard, not merely computed and discarded — the PTY original's whole
/// point, per its own comment.
#[test]
fn an_action_on_the_dashboard_reaches_the_selected_project() {
    let radar = radar_of(vec![project("alpha-02", "alpha-02")]);
    let mut screen = Screen::Dashboard;
    let mut dashboard_state = Some(DashboardState::with_collapsed(
        &radar,
        Prefs::default().collapsed,
    ));
    let mut browser_state: Option<petri::browser::BrowserState> = None;
    let mut picker = None;
    let mut picker_action = None;
    let mut help_open = false;
    let mut notice = None;
    let mut prefs = Prefs::default();
    let prefs_path = scratch_prefs_path("action_dispatch");
    let last_good = Some(radar);
    let feed = petri::feed::FeedState::default();
    let mut terminal =
        Terminal::new(TestBackend::new(80, 24)).expect("TestBackend terminal must construct");

    handle_key(
        key(KeyCode::Char('j')),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );

    let outcome = handle_key(
        key(KeyCode::Char('o')),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );
    assert_eq!(outcome, KeyOutcome::Continue(true));
    assert_eq!(
        notice.as_deref(),
        Some("alpha-02 has no remote"),
        "an action key on the Dashboard must reach begin_action (issue #64)"
    );

    render_current(
        &mut terminal,
        &last_good,
        screen,
        &dashboard_state,
        &browser_state,
        &picker,
        help_open,
        &notice,
        &feed,
        &prefs,
    );
    let text = rendered_text(&terminal);
    assert!(
        text.contains("no remote"),
        "the notice must be drawn, not just computed, got:\n{text}"
    );
}

/// Replaces `s13_pty_dashboard_actions.rs`'s
/// `an_action_still_fires_while_the_focus_popup_is_open`.
///
/// The focus popup has no key handling of its own — it renders whatever
/// `DashboardState::selected` points at — so a project row under an open
/// popup must dispatch exactly like a closed one.
#[test]
fn an_action_still_fires_while_the_focus_popup_is_open() {
    let radar = radar_of(vec![project("alpha-02", "alpha-02")]);
    let mut screen = Screen::Dashboard;
    let mut dashboard_state = Some(DashboardState::with_collapsed(
        &radar,
        Prefs::default().collapsed,
    ));
    let mut browser_state: Option<petri::browser::BrowserState> = None;
    let mut picker = None;
    let mut picker_action = None;
    let mut help_open = false;
    let mut notice = None;
    let mut prefs = Prefs::default();
    let prefs_path = scratch_prefs_path("action_popup");
    let last_good = Some(radar);
    let mut terminal =
        Terminal::new(TestBackend::new(80, 24)).expect("TestBackend terminal must construct");

    handle_key(
        key(KeyCode::Char('j')),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );
    handle_key(
        key(KeyCode::Char(' ')),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );
    assert!(
        dashboard_state.as_ref().is_some_and(|d| d.focus_open),
        "precondition: the focus popup must be open"
    );

    let outcome = handle_key(
        key(KeyCode::Char('o')),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );
    assert_eq!(outcome, KeyOutcome::Continue(true));
    assert_eq!(
        notice.as_deref(),
        Some("alpha-02 has no remote"),
        "an action key must still dispatch to the selected project while the focus popup is open (issue #64)"
    );
}

/// Replaces `s8_pty_help.rs`'s `help_popup_opens_and_closes_on_any_key`.
///
/// The popup is a modal that consumes every keystroke while open — `?` opens
/// it, and any subsequent key (even one with its own normal-mode binding,
/// `j` here) closes it without falling through to that binding. Starts
/// directly on the Browser screen rather than reaching it via `Tab`: `Tab`'s
/// own screen-switch persistence is exercised by
/// `enter_on_a_dashboard_row_switches_to_browser` above, and this test's
/// property is the popup, not how the Browser was reached.
#[test]
fn help_popup_opens_and_closes_on_any_key() {
    let radar = radar_of(vec![project("alpha", "alpha")]);
    let mut screen = Screen::Browser;
    let mut dashboard_state: Option<DashboardState> = None;
    let mut browser_state = Some(petri::browser::BrowserState::new(&radar));
    let mut picker = None;
    let mut picker_action = None;
    let mut help_open = false;
    let mut notice = None;
    let mut prefs = Prefs::default();
    let prefs_path = scratch_prefs_path("help_popup");
    let last_good = Some(radar);
    let feed = petri::feed::FeedState::default();
    let mut terminal =
        Terminal::new(TestBackend::new(80, 24)).expect("TestBackend terminal must construct");

    let after_question = handle_key(
        key(KeyCode::Char('?')),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );
    assert_eq!(after_question, KeyOutcome::Continue(true));
    assert!(help_open, "'?' must open the help popup");

    render_current(
        &mut terminal,
        &last_good,
        screen,
        &dashboard_state,
        &browser_state,
        &picker,
        help_open,
        &notice,
        &feed,
        &prefs,
    );
    let opened_text = rendered_text(&terminal);
    assert!(
        opened_text.contains("any key closes"),
        "the help popup's own footer must be drawn while open, got:\n{opened_text}"
    );

    // `j` has its own normal-mode binding (move selection) — closing on it
    // rather than falling through is the whole point of a modal popup.
    let after_j = handle_key(
        key(KeyCode::Char('j')),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );
    assert_eq!(after_j, KeyOutcome::Continue(true));
    assert!(
        !help_open,
        "any key, including one with its own binding, must close the help popup"
    );
}

/// Shared setup for the focus-popup tests below: a Dashboard with three
/// RUNNING (`StatusBucket::Active`) projects, so a header collapse has
/// visible rows to remove and `j` lands the cursor on a real project row.
/// Returns `(radar, screen, dashboard_state)`, with `screen`/`dashboard_state`
/// already the `Dashboard` starting point every PTY original assumed.
fn focus_test_state() -> (Radar, Screen, Option<DashboardState>) {
    let radar = radar_of(vec![
        project("alpha-01", "alpha-01"),
        project("alpha-02", "alpha-02"),
        project("alpha-03", "alpha-03"),
    ]);
    let dashboard_state = Some(DashboardState::with_collapsed(
        &radar,
        Prefs::default().collapsed,
    ));
    (radar, Screen::Dashboard, dashboard_state)
}

/// Replaces `s11_pty_focus.rs`'s `space_on_a_project_row_opens_the_focus_popup`.
#[test]
fn space_on_a_project_row_opens_the_focus_popup() {
    let (radar, mut screen, mut dashboard_state) = focus_test_state();
    let mut browser_state: Option<petri::browser::BrowserState> = None;
    let mut picker = None;
    let mut picker_action = None;
    let mut help_open = false;
    let mut notice = None;
    let mut prefs = Prefs::default();
    let prefs_path = scratch_prefs_path("focus_open");
    let last_good = Some(radar);
    let mut terminal =
        Terminal::new(TestBackend::new(80, 24)).expect("TestBackend terminal must construct");

    assert!(
        !dashboard_state.as_ref().unwrap().focus_open,
        "precondition: no popup before any keystroke"
    );

    // `j` first: the cursor lands on the RUNNING header, and `Space` there is
    // the unchanged collapse binding — a project row needs one `j`.
    handle_key(
        key(KeyCode::Char('j')),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );
    handle_key(
        key(KeyCode::Char(' ')),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );

    assert!(
        dashboard_state.as_ref().unwrap().focus_open,
        "Space on a project row must open the focus popup (issue #30)"
    );
}

/// Replaces `s11_pty_focus.rs`'s `esc_closes_the_focus_popup`.
#[test]
fn esc_closes_the_focus_popup() {
    let (radar, mut screen, mut dashboard_state) = focus_test_state();
    let mut browser_state: Option<petri::browser::BrowserState> = None;
    let mut picker = None;
    let mut picker_action = None;
    let mut help_open = false;
    let mut notice = None;
    let mut prefs = Prefs::default();
    let prefs_path = scratch_prefs_path("focus_esc");
    let last_good = Some(radar);
    let mut terminal =
        Terminal::new(TestBackend::new(80, 24)).expect("TestBackend terminal must construct");

    for k in [KeyCode::Char('j'), KeyCode::Char(' ')] {
        handle_key(
            key(k),
            &mut terminal,
            &mut screen,
            &mut dashboard_state,
            &mut browser_state,
            &mut picker,
            &mut picker_action,
            &mut help_open,
            &mut notice,
            &last_good,
            &mut prefs,
            &prefs_path,
        );
    }
    assert!(
        dashboard_state.as_ref().unwrap().focus_open,
        "precondition: the popup must be open"
    );

    handle_key(
        key(KeyCode::Esc),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );
    assert!(
        !dashboard_state.as_ref().unwrap().focus_open,
        "Esc must close the focus popup"
    );
    assert_eq!(
        screen,
        Screen::Dashboard,
        "Esc must leave the Dashboard as the active screen"
    );
}

/// Replaces `s11_pty_focus.rs`'s `space_twice_on_a_row_closes_what_it_opened`.
#[test]
fn space_twice_on_a_row_closes_what_it_opened() {
    let (radar, mut screen, mut dashboard_state) = focus_test_state();
    let mut browser_state: Option<petri::browser::BrowserState> = None;
    let mut picker = None;
    let mut picker_action = None;
    let mut help_open = false;
    let mut notice = None;
    let mut prefs = Prefs::default();
    let prefs_path = scratch_prefs_path("focus_toggle");
    let last_good = Some(radar);
    let mut terminal =
        Terminal::new(TestBackend::new(80, 24)).expect("TestBackend terminal must construct");

    for k in [KeyCode::Char('j'), KeyCode::Char(' ')] {
        handle_key(
            key(k),
            &mut terminal,
            &mut screen,
            &mut dashboard_state,
            &mut browser_state,
            &mut picker,
            &mut picker_action,
            &mut help_open,
            &mut notice,
            &last_good,
            &mut prefs,
            &prefs_path,
        );
    }
    assert!(
        dashboard_state.as_ref().unwrap().focus_open,
        "precondition: the popup must be open"
    );

    handle_key(
        key(KeyCode::Char(' ')),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );
    assert!(
        !dashboard_state.as_ref().unwrap().focus_open,
        "a second Space on a row must close the popup it opened"
    );
}

/// Replaces `s11_pty_focus.rs`'s `space_on_a_header_still_collapses_the_section`.
#[test]
fn space_on_a_header_still_collapses_the_section() {
    let (radar, mut screen, mut dashboard_state) = focus_test_state();
    let mut browser_state: Option<petri::browser::BrowserState> = None;
    let mut picker = None;
    let mut picker_action = None;
    let mut help_open = false;
    let mut notice = None;
    let mut prefs = Prefs::default();
    let prefs_path = scratch_prefs_path("focus_header_collapse");
    let last_good = Some(radar);
    let mut terminal =
        Terminal::new(TestBackend::new(80, 24)).expect("TestBackend terminal must construct");

    // No `j`: the cursor starts on the RUNNING header (`DashboardState::rebuild`'s
    // first stop), so `Space` here targets the header, not a row.
    let rows_before = dashboard_state
        .as_ref()
        .unwrap()
        .visible
        .iter()
        .filter(|r| matches!(r, petri::dashboard::DashRow::Project(_)))
        .count();
    assert!(rows_before > 0, "precondition: RUNNING has project rows");

    handle_key(
        key(KeyCode::Char(' ')),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );

    let state = dashboard_state.as_ref().unwrap();
    let rows_after = state
        .visible
        .iter()
        .filter(|r| matches!(r, petri::dashboard::DashRow::Project(_)))
        .count();
    assert!(
        rows_after < rows_before,
        "Space on the RUNNING header must still collapse it: before {rows_before}, after {rows_after}"
    );
    assert!(
        !state.focus_open,
        "collapsing a header must NOT open the focus popup"
    );
}

/// Replaces `s11_pty_focus.rs`'s
/// `a_small_terminal_renders_the_panel_full_screen_instead_of_a_popup`.
///
/// At 48x14 `focus_placement` (graded as arithmetic by `s11_focus_mount.rs`)
/// decides the panel takes the whole frame rather than overlaying a popup —
/// this proves the mount actually honours that decision when driven through
/// real dispatch, the same property the PTY original proved through a real
/// terminal resize.
#[test]
fn a_small_terminal_renders_the_panel_full_screen_instead_of_a_popup() {
    const W: u16 = 48;
    const H: u16 = 14;
    let (radar, mut screen, mut dashboard_state) = focus_test_state();
    let mut browser_state: Option<petri::browser::BrowserState> = None;
    let mut picker = None;
    let mut picker_action = None;
    let mut help_open = false;
    let mut notice = None;
    let mut prefs = Prefs::default();
    let prefs_path = scratch_prefs_path("focus_fullscreen");
    let last_good = Some(radar);
    let feed = petri::feed::FeedState::default();
    let mut terminal =
        Terminal::new(TestBackend::new(W, H)).expect("TestBackend terminal must construct");

    for k in [KeyCode::Char('j'), KeyCode::Char(' ')] {
        handle_key(
            key(k),
            &mut terminal,
            &mut screen,
            &mut dashboard_state,
            &mut browser_state,
            &mut picker,
            &mut picker_action,
            &mut help_open,
            &mut notice,
            &last_good,
            &mut prefs,
            &prefs_path,
        );
    }
    assert!(dashboard_state.as_ref().unwrap().focus_open);

    render_current(
        &mut terminal,
        &last_good,
        screen,
        &dashboard_state,
        &browser_state,
        &picker,
        help_open,
        &notice,
        &feed,
        &prefs,
    );
    let text = rendered_text(&terminal);
    let row0 = text.lines().next().unwrap_or("");
    assert!(
        !row0.contains("dashboard"),
        "at 48x14 the panel must take the whole frame, not overlay it — the Dashboard's own \
         header must be gone from row 0, got: {row0:?}"
    );
    assert!(
        !text.contains("Focus"),
        "the full-screen render must carry no popup border/title, got:\n{text}"
    );
    assert!(
        text.contains("alpha-"),
        "the panel itself must have rendered its project, got:\n{text}"
    );
}

/// Replaces `s8_pty_actions.rs`'s `action_keys_do_not_fire_while_the_filter_has_focus`.
///
/// `g` is bound to git history in normal mode. Typed into the `/` filter it must be a
/// filter character and nothing else — `lib.rs` keeps the two key branches structurally
/// separate so a printable action key can never be stolen while the user is typing.
#[test]
fn action_keys_do_not_fire_while_the_filter_has_focus() {
    let radar = radar_of(vec![project("alpha-01", "alpha-01")]);
    let mut screen = Screen::Browser;
    let mut dashboard_state: Option<DashboardState> = None;
    let mut browser_state = Some(petri::browser::BrowserState::new(&radar));
    let mut picker = None;
    let mut picker_action = None;
    let mut help_open = false;
    let mut notice = None;
    let mut prefs = Prefs::default();
    let prefs_path = scratch_prefs_path("filter_swallows_action");
    let last_good = Some(radar);
    let mut terminal =
        Terminal::new(TestBackend::new(80, 24)).expect("TestBackend terminal must construct");

    handle_key(
        key(KeyCode::Char('/')),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );
    assert!(
        browser_state.as_ref().unwrap().filter_input,
        "precondition: '/' must open filter input mode"
    );

    handle_key(
        key(KeyCode::Char('g')),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );

    let state = browser_state.as_ref().unwrap();
    assert_eq!(
        state.filter_query, "g",
        "'g' typed while the filter has focus must append to the query, not dispatch the \
         git-history action"
    );
    assert!(
        state.visible.is_empty(),
        "no fixture project name contains 'g', so the filter must have emptied the visible \
         list — a 'g' stolen by the action dispatch would leave it unfiltered"
    );
    assert_eq!(
        notice, None,
        "the action dispatch must never have fired — no notice should be set"
    );
}

/// Replaces `s8_pty_actions.rs`'s `an_action_on_a_project_with_no_remote_reports_it`.
#[test]
fn an_action_on_a_project_with_no_remote_reports_it() {
    let radar = radar_of(vec![project("alpha-02", "alpha-02")]);
    let mut screen = Screen::Browser;
    let mut dashboard_state: Option<DashboardState> = None;
    let mut browser_state = Some(petri::browser::BrowserState::new(&radar));
    let mut picker = None;
    let mut picker_action = None;
    let mut help_open = false;
    let mut notice = None;
    let mut prefs = Prefs::default();
    let prefs_path = scratch_prefs_path("browser_no_remote_notice");
    let last_good = Some(radar);
    let mut terminal =
        Terminal::new(TestBackend::new(80, 24)).expect("TestBackend terminal must construct");

    let outcome = handle_key(
        key(KeyCode::Char('o')),
        &mut terminal,
        &mut screen,
        &mut dashboard_state,
        &mut browser_state,
        &mut picker,
        &mut picker_action,
        &mut help_open,
        &mut notice,
        &last_good,
        &mut prefs,
        &prefs_path,
    );
    assert_eq!(outcome, KeyOutcome::Continue(true));
    assert_eq!(
        notice.as_deref(),
        Some("alpha-02 has no remote"),
        "ACT-9's per-project availability axis must produce a notice naming the project"
    );
}
