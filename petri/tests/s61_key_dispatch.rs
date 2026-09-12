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
//! - `s10_pty_reload.rs`'s `a_state_file_reload_does_not_reopen_a_collapsed_section` — the
//!   suite's single slowest test (7.7s: a real 7s sleep to wait out `poll_loop`'s poll
//!   interval before asserting on the reload). Its actual property — that a reload
//!   preserves the user's collapsed sections — lives entirely in `reload_if_changed`
//!   (extracted from `poll_loop` the same way `handle_key` was), which this file now calls
//!   directly against a real scratch state file: no terminal, no polling, no 7s wait.
//! - `s8_pty_filter.rs`'s two tests (`the_typed_query_appears_on_screen_and_survives_enter`,
//!   `backspace_deletes_the_last_character_and_refilters`) — that file is now empty, deleted.
//! - `s8_pty_repick.rs`'s three tests (`shift_g_opens_the_repick_popup_even_when_the_choice_already_resolves`,
//!   `shift_g_does_not_fire_while_the_filter_has_focus`,
//!   `a_shifted_key_on_an_action_with_no_target_reports_it_instead_of_popping`) — that file
//!   is now empty, deleted. The regression it guards (`Esc` in re-pick mode must not write
//!   `petri.toml`) is provable in-process the same way it was over a real subprocess: seed a
//!   scratch prefs file, press `G` then `Esc`, and diff the bytes.
//! - `s8_pty_prefs.rs`'s `switching_screens_preserves_stored_tool_choices` — that file is now
//!   empty, deleted.
//! - The dispatch half of `s7_pty.rs`'s `tab_switches_dashboard_to_browser_and_back` (`Tab`
//!   round-tripping Dashboard→Browser→Dashboard and persisting each switch). That file's
//!   other two tests pin `run`'s own startup sequence, not `handle_key`'s, and stay PTY.
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
use petri::{KeyOutcome, Screen, handle_key, reload_if_changed, render_current};
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

fn project_in(id: &str, name: &str, bucket: StatusBucket) -> Project {
    Project {
        status_bucket: bucket,
        ..project(id, name)
    }
}

/// A project that IS a git repository (but still has no `github_url`) — for `gitlog`
/// (`Target::GitRepo`) dispatch, which `project`'s plain `GitState::not_a_repo()` would
/// always report as `NoTarget` for, popup or no popup.
fn project_repo(id: &str, name: &str) -> Project {
    Project {
        git: GitState {
            is_repo: true,
            ..GitState::not_a_repo()
        },
        ..project(id, name)
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

/// Replaces `s10_pty_reload.rs`'s
/// `a_state_file_reload_does_not_reopen_a_collapsed_section`.
///
/// The regression: `poll_loop` used to rebuild the Dashboard with
/// `DashboardState::new` on every mtime change, which hardcodes the spec's
/// *default* collapse state — so a user's own collapsed section reopened
/// itself on the next scan, with no input from them. The fix was `refresh`
/// preserving the caller's existing `collapsed` state instead; this test
/// pins that property directly against `reload_if_changed`, the function
/// `poll_loop`'s reload branch was extracted into (issue #61).
///
/// IN FLIGHT is seeded collapsed alongside the two that ship collapsed by
/// default, so it is the ONLY thing distinguishing this from
/// `DashboardState::with_collapsed`'s own defaults — if a reload reset to
/// `[false, false, true, true]`, index 1 would flip back and this is the
/// property that would go undetected. `scan_duration_ms` changing between
/// the two writes is this test's proof the reload actually read the new
/// file rather than silently no-op'ing on a stale one.
#[test]
fn a_state_file_reload_does_not_reopen_a_collapsed_section() {
    // Whether any IN FLIGHT project's row is visible in `dashboard_state.visible` — a
    // collapsed section keeps its header row but drops its `DashRow::Project` rows, so
    // this is what actually distinguishes collapsed from expanded.
    fn in_flight_rows_visible(radar: &Radar, state: &DashboardState) -> bool {
        state.visible.iter().any(|r| match r {
            petri::dashboard::DashRow::Project(i) => {
                radar.projects[*i].status_bucket == StatusBucket::InFlight
            }
            petri::dashboard::DashRow::Header(_) => false,
        })
    }

    let dir = std::env::temp_dir().join(format!("petri_s61_reload_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir must be creatable");
    let state_path = dir.join("radar.json");

    let in_flight_members = vec![
        project_in("ember-core", "ember-core", StatusBucket::InFlight),
        project_in("forest-net", "forest-net", StatusBucket::InFlight),
    ];
    let before_radar = Radar {
        schema_version: SCHEMA_VERSION,
        updated_at: chrono::Utc::now(),
        scan_duration_ms: 312,
        projects: in_flight_members.clone(),
        quota: None,
    };
    std::fs::write(
        &state_path,
        serde_json::to_string(&before_radar).expect("serialize"),
    )
    .expect("initial state write must succeed");
    let last_mtime = std::fs::metadata(&state_path)
        .expect("state file must exist")
        .modified()
        .ok();

    // IN FLIGHT collapsed alongside the two that ship collapsed — the seeded
    // starting point a real session would have after the user pressed Space
    // on it once, persisted to petri.toml, and restarted.
    let collapsed = [false, true, true, true];
    let mut dashboard_state = Some(DashboardState::with_collapsed(&before_radar, collapsed));
    assert!(
        !in_flight_rows_visible(&before_radar, dashboard_state.as_ref().unwrap()),
        "precondition: IN FLIGHT must start collapsed, so none of its rows are visible"
    );

    let mut last_good = Some(before_radar);
    let mut browser_state: Option<petri::browser::BrowserState> = None;
    let mut feed = petri::feed::FeedState::default();
    let prefs = Prefs {
        collapsed,
        ..Prefs::default()
    };

    // A real mtime change needs real, distinguishable filesystem time — not this
    // function's problem to work around, just a fact of the fixture. Far cheaper
    // than the 7s the PTY original slept to also wait out poll_loop's own poll
    // interval, which reload_if_changed has none of: called directly, there is no
    // interval to wait out.
    std::thread::sleep(std::time::Duration::from_millis(1100));

    let after_radar = Radar {
        schema_version: SCHEMA_VERSION,
        updated_at: chrono::Utc::now(),
        scan_duration_ms: 9900,
        projects: in_flight_members,
        quota: None,
    };
    std::fs::write(
        &state_path,
        serde_json::to_string(&after_radar).expect("serialize"),
    )
    .expect("state rewrite must succeed");

    let (_new_mtime, mtime_changed) = reload_if_changed(
        &state_path,
        last_mtime,
        &mut last_good,
        &mut dashboard_state,
        &mut browser_state,
        &mut feed,
        &prefs,
    );

    assert!(mtime_changed, "the mtime change must have been detected");
    let last_good = last_good.expect("a successful reload must leave last_good populated");
    let dashboard_state =
        dashboard_state.expect("a successful reload must not clear dashboard_state");
    assert_eq!(
        last_good.scan_duration_ms, 9900,
        "the reload never landed, so this test proves nothing about collapse"
    );
    assert_eq!(
        dashboard_state.collapsed, collapsed,
        "a reload must preserve the user's collapsed sections, not reset to defaults"
    );
    assert!(
        !in_flight_rows_visible(&last_good, &dashboard_state),
        "a reload reopened the collapsed IN FLIGHT section — its rows are back, i.e. the \
         user's layout changed with no input from them"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Shared per-test dispatch state for the Browser-screen tests below, bundled into a
/// struct only so `press` can take `&mut self` instead of eleven separate `&mut`
/// parameters repeated at every call site — see `render_current`'s own doc comment for
/// why the rest of this file doesn't do this: those functions have no natural grouping,
/// but this one struct exists only inside a handful of tests and dies with them.
struct BrowserHarness {
    screen: Screen,
    dashboard_state: Option<DashboardState>,
    browser_state: Option<petri::browser::BrowserState>,
    picker: Option<petri::picker::PickerState>,
    picker_action: Option<petri::tools::Action>,
    help_open: bool,
    notice: Option<String>,
    prefs: Prefs,
    prefs_path: std::path::PathBuf,
    last_good: Option<Radar>,
}

impl BrowserHarness {
    fn browser(radar: Radar, tag: &str) -> Self {
        Self::browser_with_prefs(radar, tag, Prefs::default())
    }

    /// Same as [`Self::browser`], but with a caller-supplied starting `Prefs` — for tests
    /// that need a stored tool choice already in place (`prefs.tools`) before the first
    /// keystroke, the same shape `seeded_home` gave the PTY originals.
    fn browser_with_prefs(radar: Radar, tag: &str, prefs: Prefs) -> Self {
        let browser_state = Some(petri::browser::BrowserState::new(&radar));
        let prefs_path = scratch_prefs_path(tag);
        petri::prefs::save(&prefs_path, &prefs).expect("seed prefs must be writable");
        BrowserHarness {
            screen: Screen::Browser,
            dashboard_state: None,
            browser_state,
            picker: None,
            picker_action: None,
            help_open: false,
            notice: None,
            prefs,
            prefs_path,
            last_good: Some(radar),
        }
    }

    fn press(&mut self, terminal: &mut Terminal<TestBackend>, code: KeyCode) -> KeyOutcome {
        handle_key(
            key(code),
            terminal,
            &mut self.screen,
            &mut self.dashboard_state,
            &mut self.browser_state,
            &mut self.picker,
            &mut self.picker_action,
            &mut self.help_open,
            &mut self.notice,
            &self.last_good,
            &mut self.prefs,
            &self.prefs_path,
        )
    }

    fn render(&self, terminal: &mut Terminal<TestBackend>) -> String {
        let feed = petri::feed::FeedState::default();
        render_current(
            terminal,
            &self.last_good,
            self.screen,
            &self.dashboard_state,
            &self.browser_state,
            &self.picker,
            self.help_open,
            &self.notice,
            &feed,
            &self.prefs,
        );
        rendered_text(terminal)
    }

    fn filter_query(&self) -> &str {
        &self.browser_state.as_ref().unwrap().filter_query
    }
}

/// Replaces `s8_pty_filter.rs`'s `the_typed_query_appears_on_screen_and_survives_enter`.
///
/// ACT-10: `/` opens the filter, typed characters both narrow `visible` and stay legible
/// on screen (the block cursor glyph marks the input as open), `Enter` closes the input
/// but keeps the query, and `Esc` in normal mode is a no-op — so the test re-opens the
/// filter and clears it there, where the chip must disappear entirely.
#[test]
fn the_typed_query_appears_on_screen_and_survives_enter() {
    const INPUT_CURSOR: char = '\u{2588}';
    let radar = radar_of(vec![
        project("alpha-01", "alpha-01"),
        project("beta-02", "beta-02"),
    ]);
    let mut h = BrowserHarness::browser(radar, "filter_query_visible");
    let mut terminal =
        Terminal::new(TestBackend::new(90, 24)).expect("TestBackend terminal must construct");

    h.press(&mut terminal, KeyCode::Char('/'));
    for c in "beta".chars() {
        h.press(&mut terminal, KeyCode::Char(c));
    }
    assert_eq!(h.filter_query(), "beta");

    let typing = h.render(&mut terminal);
    assert!(
        typing.contains("/beta"),
        "the query typed into the `/` filter must be visible, got:\n{typing}"
    );
    assert!(
        typing.contains(INPUT_CURSOR),
        "the block cursor must mark the filter as open, got:\n{typing}"
    );

    h.press(&mut terminal, KeyCode::Enter);
    assert!(
        !h.browser_state.as_ref().unwrap().filter_input,
        "Enter must close the filter input"
    );
    assert_eq!(
        h.filter_query(),
        "beta",
        "Enter must keep the query, not discard it (petri/SPEC.md §3.1)"
    );

    let kept = h.render(&mut terminal);
    assert!(
        kept.contains("/beta"),
        "the query must remain visible after Enter closes the input, got:\n{kept}"
    );
    assert!(
        !kept.contains(INPUT_CURSOR),
        "the block cursor must be gone once the input is closed, got:\n{kept}"
    );

    // Esc in normal mode is a no-op, so re-open the filter and clear it there.
    h.press(&mut terminal, KeyCode::Char('/'));
    h.press(&mut terminal, KeyCode::Esc);
    assert_eq!(
        h.filter_query(),
        "",
        "Esc must clear the query, taking the chip with it"
    );
}

/// Replaces `s8_pty_filter.rs`'s `backspace_deletes_the_last_character_and_refilters`.
///
/// Backspace arrives as a real terminal byte (0x7f, DEL) in a real terminal, but the
/// dispatch it triggers — drop the last character and re-apply the filter — is plain
/// state, provable directly against `handle_key` without one.
#[test]
fn backspace_deletes_the_last_character_and_refilters() {
    let radar = radar_of(vec![project("bravo-01", "bravo-01")]);
    let mut h = BrowserHarness::browser(radar, "filter_backspace");
    let mut terminal =
        Terminal::new(TestBackend::new(90, 24)).expect("TestBackend terminal must construct");

    h.press(&mut terminal, KeyCode::Char('/'));
    for c in "bravoX".chars() {
        h.press(&mut terminal, KeyCode::Char(c));
    }
    assert_eq!(h.filter_query(), "bravoX");
    assert!(
        h.browser_state.as_ref().unwrap().visible.is_empty(),
        "setup: \"bravoX\" must match nothing, or the delete proves nothing"
    );

    h.press(&mut terminal, KeyCode::Backspace);
    assert_eq!(
        h.filter_query(),
        "bravo",
        "Backspace must drop exactly the last character"
    );
    assert!(
        !h.browser_state.as_ref().unwrap().visible.is_empty(),
        "the list must re-filter on Backspace, not just redraw the query — \"bravo\" \
         matches bravo-01"
    );

    // Backspacing past the start is a no-op, not a panic or an underflow.
    for _ in 0..8 {
        h.press(&mut terminal, KeyCode::Backspace);
    }
    assert_eq!(
        h.filter_query(),
        "",
        "Backspace past the start of the query must not underflow"
    );
    assert_eq!(
        h.screen,
        Screen::Browser,
        "petri must survive Backspace on an empty query"
    );
}

/// Replaces `s8_pty_repick.rs`'s
/// `shift_g_opens_the_repick_popup_even_when_the_choice_already_resolves`.
///
/// **Inverts `ACT-8`'s "tests must never see the picker"** on purpose: the picker *is*
/// the feature here, since a shifted key must open the re-pick popup even when the
/// lowercase key would resolve cleanly (this test seeds `tools.gitlog = "serie"` so it
/// would). `Esc` is specified to launch nothing and change nothing — the strongest
/// available assertion, so this diffs the seeded prefs file byte-for-byte across the
/// press. The PTY original needed a real subprocess only to prove `lib.rs` actually
/// honoured `persist: false` before touching the file; `prefs_path` being an explicit
/// parameter now (see this module's doc comment) makes that provable directly.
#[test]
fn shift_g_opens_the_repick_popup_even_when_the_choice_already_resolves() {
    let radar = radar_of(vec![project_repo("alpha-01", "alpha-01")]);
    let mut prefs = Prefs::default();
    prefs
        .tools
        .insert("gitlog".to_string(), "serie".to_string());
    let mut h = BrowserHarness::browser_with_prefs(radar, "repick_opens", prefs);
    let mut terminal =
        Terminal::new(TestBackend::new(90, 24)).expect("TestBackend terminal must construct");

    let before = std::fs::read(&h.prefs_path).expect("seeded prefs must exist");

    h.press(&mut terminal, KeyCode::Char('G'));
    assert!(
        h.picker.is_some(),
        "G must open the re-pick popup even though gitlog already resolves"
    );

    let opened = h.render(&mut terminal);
    assert!(
        opened.contains("git history"),
        "the popup must be titled with the action, got:\n{opened}"
    );
    assert!(
        opened.contains("this time"),
        "the popup must frame itself as a one-off, not a settings dialog:\n{opened}"
    );
    assert!(
        opened.contains("run once"),
        "the popup must advertise the one-off verb:\n{opened}"
    );
    assert!(
        opened.contains("D set default"),
        "the popup must advertise the re-default verb:\n{opened}"
    );

    h.press(&mut terminal, KeyCode::Esc);
    assert!(h.picker.is_none(), "Esc must close the popup");

    let after = std::fs::read(&h.prefs_path).expect("prefs must still exist");
    assert_eq!(
        after, before,
        "opening and cancelling the re-pick popup must not touch petri.toml"
    );
    assert!(
        String::from_utf8_lossy(&after).contains("gitlog = \"serie\""),
        "the stored default must survive the popup verbatim"
    );
}

/// Replaces `s8_pty_repick.rs`'s `shift_g_does_not_fire_while_the_filter_has_focus`.
///
/// The same trap `action_keys_do_not_fire_while_the_filter_has_focus` guards for the
/// lowercase keys. `G` is an ordinary printable character, so a binding placed in the
/// wrong branch would pop a modal over a user who was typing a project name.
#[test]
fn shift_g_does_not_fire_while_the_filter_has_focus() {
    let radar = radar_of(vec![project("alpha-01", "alpha-01")]);
    let mut prefs = Prefs::default();
    prefs
        .tools
        .insert("gitlog".to_string(), "serie".to_string());
    let mut h = BrowserHarness::browser_with_prefs(radar, "repick_filter", prefs);
    let mut terminal =
        Terminal::new(TestBackend::new(90, 24)).expect("TestBackend terminal must construct");

    h.press(&mut terminal, KeyCode::Char('/'));
    h.press(&mut terminal, KeyCode::Char('G'));

    assert_eq!(
        h.filter_query(),
        "G",
        "typing G into the filter must append to the query, not dispatch the re-pick popup"
    );
    assert!(
        h.picker.is_none(),
        "typing G into the filter must not open the re-pick popup"
    );
}

/// Replaces `s8_pty_repick.rs`'s
/// `a_shifted_key_on_an_action_with_no_target_reports_it_instead_of_popping`.
///
/// Exercises the other arm of `begin_repick`: `browse` (`O`) is `Target::Url`, so on a
/// project with no `github_url` there is nothing to re-pick and the answer is `ACT-9`'s
/// per-project notice, not an empty popup. `gitlog` (`G`) on the same row still opens,
/// because it's `Target::Path` and every project has one — asserting both is what shows
/// the difference is the action's target, not the row.
#[test]
fn a_shifted_key_on_an_action_with_no_target_reports_it_instead_of_popping() {
    let radar = radar_of(vec![project_repo("alpha-02", "alpha-02")]);
    let mut h = BrowserHarness::browser(radar, "repick_no_target");
    let mut terminal =
        Terminal::new(TestBackend::new(90, 24)).expect("TestBackend terminal must construct");

    h.press(&mut terminal, KeyCode::Char('O'));
    assert_eq!(
        h.notice.as_deref(),
        Some("alpha-02 has no remote"),
        "O on a project with no remote must explain itself instead of opening a popup"
    );
    assert!(
        h.picker.is_none(),
        "there is nothing to re-pick, so no popup"
    );

    // Same row, Target::Path action: the popup does open.
    h.press(&mut terminal, KeyCode::Char('G'));
    assert!(
        h.picker.is_some(),
        "G must still open on the same project — every project has a path"
    );
}

/// Replaces `s8_pty_prefs.rs`'s `switching_screens_preserves_stored_tool_choices`.
///
/// A real bug, not a hypothetical one: adding `tools` to `Prefs` forced every
/// `Prefs { .. }` struct literal in `lib.rs` to name the new field, and each of the
/// three did so as an empty map — so every `Tab` press wrote a preferences file with
/// `[tools]` emptied. No pure test could catch it: `prefs::save` was called correctly
/// with exactly the struct it was handed; only a real key press, writing a real file,
/// shows a screen switch destroying a sibling field it never touched. Provable
/// in-process the same way, now that `prefs_path` is a parameter: seed a scratch file,
/// press Tab, read it back.
#[test]
fn switching_screens_preserves_stored_tool_choices() {
    let radar = radar_of(vec![project("alpha-01", "alpha-01")]);
    let mut screen = Screen::Dashboard;
    let mut dashboard_state: Option<DashboardState> = None;
    let mut browser_state: Option<petri::browser::BrowserState> = None;
    let mut picker = None;
    let mut picker_action = None;
    let mut help_open = false;
    let mut notice = None;
    let mut prefs = Prefs::default();
    prefs.tools.insert("edit".to_string(), "code".to_string());
    prefs
        .tools
        .insert("gitlog".to_string(), "serie".to_string());
    let prefs_path = scratch_prefs_path("tools_survive_tab");
    petri::prefs::save(&prefs_path, &prefs).expect("seed prefs must be writable");
    let last_good = Some(radar);
    let mut terminal =
        Terminal::new(TestBackend::new(80, 24)).expect("TestBackend terminal must construct");

    handle_key(
        key(KeyCode::Tab),
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
    assert_eq!(
        screen,
        Screen::Browser,
        "precondition: Tab must switch screens"
    );

    let written = std::fs::read_to_string(&prefs_path).expect("prefs file must still exist");
    assert!(
        written.contains("[tools]"),
        "the [tools] table was destroyed by a screen switch:\n{written}"
    );
    assert!(
        written.contains("edit = \"code\""),
        "the stored editor choice was lost:\n{written}"
    );
    assert!(
        written.contains("gitlog = \"serie\""),
        "the stored git-history choice was lost:\n{written}"
    );
    assert!(
        written.contains("last_screen = \"browser\""),
        "the Tab switch itself must still have been persisted:\n{written}"
    );
}

/// Replaces the dispatch half of `s7_pty.rs`'s
/// `tab_switches_dashboard_to_browser_and_back` — `Tab` round-tripping
/// Dashboard→Browser→Dashboard and persisting each switch. That file's other
/// two tests (`valid_petri_toml_is_applied_on_startup`,
/// `corrupt_petri_toml_does_not_prevent_startup`) stay PTY: they pin `run`'s
/// own startup sequence (`prefs::load` before the alternate screen is
/// entered), which is `run`'s code, not `handle_key`'s, and has no
/// `TestBackend` equivalent. This test also drops the original's trailing
/// `'q' must exit 0` assertion — a real exit code is exactly the kind of
/// property this file leaves to PTY tests elsewhere (`s4_pty.rs` already
/// covers plain `q` after startup), not something Tab's own dispatch adds.
#[test]
fn tab_switches_dashboard_to_browser_and_back() {
    let radar = radar_of(vec![project("alpha-01", "alpha-01")]);
    let mut screen = Screen::Dashboard;
    let mut dashboard_state: Option<DashboardState> = None;
    let mut browser_state: Option<petri::browser::BrowserState> = None;
    let mut picker = None;
    let mut picker_action = None;
    let mut help_open = false;
    let mut notice = None;
    let mut prefs = Prefs::default();
    let prefs_path = scratch_prefs_path("tab_round_trip");
    let last_good = Some(radar);
    let mut terminal =
        Terminal::new(TestBackend::new(80, 24)).expect("TestBackend terminal must construct");

    handle_key(
        key(KeyCode::Tab),
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
    assert_eq!(
        screen,
        Screen::Browser,
        "Tab from the Dashboard must switch to the Browser (petri/SPEC.md §5)"
    );
    assert!(
        browser_state.is_some(),
        "the Browser's state must be built on the first Tab switch"
    );
    let after_first = std::fs::read_to_string(&prefs_path).expect("prefs file must exist");
    assert!(
        after_first.contains("last_screen = \"browser\""),
        "the first Tab switch must be persisted:\n{after_first}"
    );

    handle_key(
        key(KeyCode::Tab),
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
    assert_eq!(
        screen,
        Screen::Dashboard,
        "Tab from the Browser must switch back to the Dashboard"
    );
    let after_second = std::fs::read_to_string(&prefs_path).expect("prefs file must exist");
    assert!(
        after_second.contains("last_screen = \"dashboard\""),
        "the second Tab switch must also be persisted:\n{after_second}"
    );
}
