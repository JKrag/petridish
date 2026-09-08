//! `SURF-8` mount #30 — the Dashboard's focus popup, as **pure state**
//! (`PLAN-focus-panel.md` §6, Phase C).
//!
//! **Protected. Authored by the planner, not the implementer** (`PLAN-focus-panel.md` §1
//! and §3): these tests are the spec for T5, they were written before any implementation
//! existed, and a round that changes or deletes one is an automatic revert regardless of
//! what the failing-test count does.
//!
//! `DashboardState::focus_target`, `press_space` and `close_focus` are `unimplemented!()`
//! in the Phase C scaffold, so every test here FAILS (panics) rather than errors — the
//! same convention `s11_focus_plan.rs` records for Phase A.
//!
//! **No terminal, no `TestBackend`.** Everything asserted here is key handling and popup
//! lifetime, which is state; the geometry (the `MECH-1` popup rect, the
//! ≤80%-of-terminal switch to a full-screen render) is T5's other half and is checked
//! attended by the PTY suite (§9), not here. Keeping the two apart is what lets the
//! contextual-`Space` contract be pinned without a pseudo-terminal in the loop.
//!
//! The behaviour being pinned is `PROPOSAL-focus-panel.md` §7's table. Its one real
//! change is a *removal*: `Space` on a project row today reaches past the row to toggle
//! its containing section and then relocates the cursor — the only binding in either
//! screen that acts on something other than what the cursor is on. Several tests below
//! exist purely to prove that special case is gone rather than merely joined by a new one.

use petri::dashboard::{DashRow, DashboardState};
use petri::focus::FocusTarget;
use petridish_core::schema::{Radar, StatusBucket};
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join(name)
}

fn load(name: &str) -> Radar {
    let text = std::fs::read_to_string(fixture_path(name))
        .unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("fixture {name} failed to deserialize into Radar: {e}"))
}

/// A state with every section expanded, so project rows in all four buckets are reachable
/// stops. The default collapse state (`STALE`/`COLD` collapsed) is exercised separately,
/// by the collapsed-header tests.
fn all_expanded(radar: &Radar) -> DashboardState {
    DashboardState::with_collapsed(radar, [false, false, false, false])
}

/// Park the cursor on the row for `name`, by name rather than by index — the scanner
/// re-sorts on every scan (`SPEC.md` §4.3), so a hard-coded position would be a fixture
/// detail masquerading as a test.
fn select_row(state: &mut DashboardState, radar: &Radar, name: &str) -> usize {
    let pos = state
        .visible
        .iter()
        .position(|row| match row {
            DashRow::Project(i) => radar.projects[*i].name == name,
            DashRow::Header(_) => false,
        })
        .unwrap_or_else(|| panic!("no visible row for project {name}"));
    state.selected = Some(pos);
    pos
}

/// Park the cursor on `bucket`'s header.
fn select_header(state: &mut DashboardState, bucket: StatusBucket) -> usize {
    let pos = state
        .visible
        .iter()
        .position(|row| *row == DashRow::Header(bucket))
        .unwrap_or_else(|| panic!("no visible header for {bucket:?}"));
    state.selected = Some(pos);
    pos
}

/// `radar.projects` index for a project named `name`.
fn idx_of(radar: &Radar, name: &str) -> usize {
    radar
        .projects
        .iter()
        .position(|p| p.name == name)
        .unwrap_or_else(|| panic!("no project named {name} in the fixture"))
}

/// The membership count `FocusTarget::Section` must carry, computed here independently of
/// `dashboard.rs`'s private `section_members` so the two can disagree and be caught.
/// RUNNING is the interesting one: it is not a `status_bucket` filter, it is
/// `running_membership`, which pulls in a cold worktree parent whose child is active.
fn expected_section_count(radar: &Radar, bucket: StatusBucket) -> usize {
    if bucket == StatusBucket::Active {
        DashboardState::running_membership(radar).len()
    } else {
        radar
            .projects
            .iter()
            .filter(|p| !p.is_foreign && p.status_bucket == bucket)
            .count()
    }
}

// ---------------------------------------------------------------------------
// `Space` on a header — unchanged from today (§7's first two table rows).
// ---------------------------------------------------------------------------

#[test]
fn space_on_a_header_still_collapses_that_section() {
    let radar = load("normal.json");
    let mut state = all_expanded(&radar);
    select_header(&mut state, StatusBucket::Active);

    state.press_space(&radar);

    assert!(
        state.collapsed[0],
        "Space on the RUNNING header must collapse RUNNING — §7 keeps this row unchanged"
    );
    assert!(
        !state
            .visible
            .iter()
            .any(|r| matches!(r, DashRow::Project(i) if radar.projects[*i].status_bucket == StatusBucket::Active)),
        "a collapsed RUNNING must have no project rows left"
    );
    assert!(
        !state.focus_open,
        "a header toggle must not open the focus popup"
    );
}

#[test]
fn space_on_a_collapsed_header_still_reopens_that_section() {
    // The rule `SPEC.md` §3.2 protects: headers are stops *precisely* so a collapsed
    // section is reachable, and `Space` on the header is what reopens it. This is the
    // test that would catch the focus popup being wired to headers too.
    let radar = load("normal.json");
    let mut state = DashboardState::new(&radar); // STALE/COLD collapsed by default
    select_header(&mut state, StatusBucket::Cold);

    state.press_space(&radar);

    assert!(!state.collapsed[3], "Space must expand a collapsed COLD");
    assert!(
        state
            .visible
            .iter()
            .any(|r| matches!(r, DashRow::Project(i) if radar.projects[*i].status_bucket == StatusBucket::Cold)),
        "expanding COLD must put its rows back in the cursor sequence"
    );
    assert!(!state.focus_open);
}

// ---------------------------------------------------------------------------
// `Space` on a project row — the behaviour change, stated as three separate
// assertions because it is one addition and two removals.
// ---------------------------------------------------------------------------

#[test]
fn space_on_a_project_row_opens_the_popup() {
    let radar = load("normal.json");
    let mut state = all_expanded(&radar);
    select_row(&mut state, &radar, "brisbane-engine");

    state.press_space(&radar);

    assert!(state.focus_open, "Space on a row must open the focus popup");
}

#[test]
fn space_on_a_project_row_does_not_move_the_cursor() {
    // Removal #1: today's `Space` relocates the cursor to the section header.
    let radar = load("normal.json");
    let mut state = all_expanded(&radar);
    let before = select_row(&mut state, &radar, "brisbane-engine");

    state.press_space(&radar);

    assert_eq!(
        state.selected,
        Some(before),
        "the cursor must stay on the row the popup is showing"
    );
    assert!(
        matches!(state.visible[before], DashRow::Project(i) if radar.projects[i].name == "brisbane-engine"),
        "and that row must still be the same project"
    );
}

#[test]
fn space_on_a_project_row_does_not_toggle_the_section() {
    // Removal #2: today's `Space` toggles the *containing* section.
    let radar = load("normal.json");
    let mut state = all_expanded(&radar);
    let visible_before = state.visible.clone();
    select_row(&mut state, &radar, "brisbane-engine");

    state.press_space(&radar);

    assert_eq!(
        state.collapsed,
        [false, false, false, false],
        "Space on a row must leave every section's collapse state alone"
    );
    assert_eq!(
        state.visible, visible_before,
        "and therefore must not rebuild the cursor sequence"
    );
}

#[test]
fn space_on_a_project_row_twice_closes_the_popup() {
    let radar = load("normal.json");
    let mut state = all_expanded(&radar);
    select_row(&mut state, &radar, "brisbane-engine");

    state.press_space(&radar);
    state.press_space(&radar);

    assert!(
        !state.focus_open,
        "Space is a toggle on a row: the second press closes what the first opened"
    );
    assert_eq!(
        state.collapsed,
        [false, false, false, false],
        "closing must still not touch the sections"
    );
}

#[test]
fn space_on_a_header_while_the_popup_is_open_closes_it_without_toggling() {
    // §7's last table row: "Any, popup open → close it". The popup wins over the
    // header's own binding, so a `Space` that dismisses the popup must not also collapse
    // the section behind it — one keypress, one effect.
    let radar = load("normal.json");
    let mut state = all_expanded(&radar);
    select_row(&mut state, &radar, "brisbane-engine");
    state.press_space(&radar);
    assert!(state.focus_open, "precondition: the popup is open");

    select_header(&mut state, StatusBucket::Active);
    state.press_space(&radar);

    assert!(!state.focus_open, "Space with the popup open closes it");
    assert!(
        !state.collapsed[0],
        "and must not also collapse the section the cursor happens to be on"
    );
}

// ---------------------------------------------------------------------------
// `Esc`
// ---------------------------------------------------------------------------

#[test]
fn esc_closes_an_open_popup_and_reports_the_key_consumed() {
    let radar = load("normal.json");
    let mut state = all_expanded(&radar);
    select_row(&mut state, &radar, "brisbane-engine");
    state.press_space(&radar);

    assert!(
        state.close_focus(),
        "Esc must report itself consumed when it closed the popup"
    );
    assert!(!state.focus_open);
}

#[test]
fn esc_with_no_popup_open_is_not_consumed() {
    // So `lib.rs` can fall through to whatever else `Esc` means without a special case.
    let radar = load("normal.json");
    let mut state = all_expanded(&radar);
    select_row(&mut state, &radar, "brisbane-engine");

    assert!(!state.close_focus());
    assert!(!state.focus_open);
}

// ---------------------------------------------------------------------------
// `focus_target` — what the popup is pointed at
// ---------------------------------------------------------------------------

#[test]
fn focus_target_on_a_project_row_is_that_project() {
    let radar = load("normal.json");
    let mut state = all_expanded(&radar);
    select_row(&mut state, &radar, "glacier-db");

    assert_eq!(
        state.focus_target(&radar),
        FocusTarget::Project(idx_of(&radar, "glacier-db"))
    );
}

#[test]
fn focus_target_on_the_running_header_carries_its_membership_count() {
    let radar = load("normal.json");
    let mut state = all_expanded(&radar);
    select_header(&mut state, StatusBucket::Active);

    assert_eq!(
        state.focus_target(&radar),
        FocusTarget::Section(
            StatusBucket::Active,
            expected_section_count(&radar, StatusBucket::Active)
        ),
        "RUNNING's count is `running_membership`, not a `status_bucket` filter"
    );
}

#[test]
fn focus_target_on_a_collapsed_header_still_counts_the_rows_it_hides() {
    let radar = load("normal.json");
    let mut state = DashboardState::new(&radar); // COLD collapsed
    assert!(state.collapsed[3], "precondition: COLD starts collapsed");
    select_header(&mut state, StatusBucket::Cold);

    assert_eq!(
        state.focus_target(&radar),
        FocusTarget::Section(
            StatusBucket::Cold,
            expected_section_count(&radar, StatusBucket::Cold)
        ),
        "a collapsed header must report how many projects it is hiding, not zero"
    );
}

#[test]
fn focus_target_with_nothing_selected_is_nothing() {
    // An empty radar is the one state with no stops at all — `rebuild` sets
    // `selected: None`, and the panel must render the empty state rather than panic
    // (the `SPEC.md` §3.1 requirement, extended to the Dashboard).
    let mut radar = load("normal.json");
    radar.projects.clear();
    let state = all_expanded(&radar);

    assert!(state.visible.is_empty(), "precondition: no stops");
    assert_eq!(state.selected, None);
    assert_eq!(state.focus_target(&radar), FocusTarget::Nothing);
}

// ---------------------------------------------------------------------------
// The popup follows the cursor
// ---------------------------------------------------------------------------

#[test]
fn the_open_popup_follows_j_and_k() {
    let radar = load("normal.json");
    let mut state = all_expanded(&radar);
    let first = select_row(&mut state, &radar, "alpha-project");
    state.press_space(&radar);
    let before = state.focus_target(&radar);

    state.move_selection(1);

    assert!(state.focus_open, "moving must not close the popup");
    assert_ne!(
        state.focus_target(&radar),
        before,
        "the panel is re-derived from the cursor on every frame, never cached"
    );
    assert_eq!(
        state.focus_target(&radar),
        match state.visible[first + 1] {
            DashRow::Project(i) => FocusTarget::Project(i),
            DashRow::Header(b) => FocusTarget::Section(b, expected_section_count(&radar, b)),
        }
    );
}

#[test]
fn moving_onto_a_header_with_the_popup_open_shows_the_section_state() {
    // The reason `FocusTarget` is not a bare `usize`: a cursor-following popup lands on a
    // non-project stop on essentially every `j`/`k` walk, and that must be a rendered
    // empty state (T1b), not a closed popup and not a panic.
    let radar = load("normal.json");
    let mut state = all_expanded(&radar);
    select_row(&mut state, &radar, "brisbane-engine");
    state.press_space(&radar);

    select_header(&mut state, StatusBucket::InFlight);

    assert!(state.focus_open, "the popup stays open over a header");
    assert_eq!(
        state.focus_target(&radar),
        FocusTarget::Section(
            StatusBucket::InFlight,
            expected_section_count(&radar, StatusBucket::InFlight)
        )
    );
}

// ---------------------------------------------------------------------------
// The other two fixtures
// ---------------------------------------------------------------------------

#[test]
fn minimal_fixture_single_project_is_reachable_and_focusable() {
    // One cold project: COLD starts collapsed, so this walks the whole gesture —
    // Space on the header to expand, `j` onto the row, Space to focus.
    let radar = load("minimal.json");
    let mut state = DashboardState::new(&radar);
    select_header(&mut state, StatusBucket::Cold);

    assert_eq!(
        state.focus_target(&radar),
        FocusTarget::Section(StatusBucket::Cold, 1)
    );

    state.press_space(&radar); // expand
    state.move_selection(1); // onto the row
    state.press_space(&radar); // focus it

    assert!(state.focus_open);
    assert_eq!(state.focus_target(&radar), FocusTarget::Project(0));
}

#[test]
fn hostile_fixture_focuses_without_panicking() {
    // A 190-character name and a CJK name. Nothing here formats anything, but the mount
    // is the layer that decides *which* project the renderer is handed, and handing it
    // the pathological one is free.
    let radar = load("hostile.json");
    let mut state = all_expanded(&radar);
    let long = radar
        .projects
        .iter()
        .max_by_key(|p| p.name.chars().count())
        .expect("hostile.json has projects")
        .name
        .clone();
    select_row(&mut state, &radar, &long);

    state.press_space(&radar);

    assert!(state.focus_open);
    assert_eq!(
        state.focus_target(&radar),
        FocusTarget::Project(idx_of(&radar, &long))
    );
}
