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

use petri::dashboard::{DashRow, DashboardState, FocusPlacement, focus_placement};
use petri::focus::FocusTarget;
use petridish_core::schema::{Radar, StatusBucket};
use ratatui::layout::Rect;
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
    // `normal.json` has no `parent_path` anywhere, so on that fixture alone
    // `running_membership` and a plain `status_bucket == active` filter give the same
    // number — and a test that cannot tell the two rules apart does not pin either. So
    // the pull-in case is built here: a cold project with an active worktree child is in
    // RUNNING (display-only, its own bucket is untouched), which makes RUNNING's count
    // one higher than the filter's. The `assert_ne!` below is a guard on the fixture, not
    // on the code: if it ever fires, this test has gone vacuous.
    let mut radar = load("normal.json");
    let cold_parent_path = radar.projects[idx_of(&radar, "mocha-py")].path.clone();
    let child = idx_of(&radar, "alpha-project");
    radar.projects[child].parent_path = Some(cold_parent_path);

    let plain_filter = radar
        .projects
        .iter()
        .filter(|p| !p.is_foreign && p.status_bucket == StatusBucket::Active)
        .count();
    assert_ne!(
        expected_section_count(&radar, StatusBucket::Active),
        plain_filter,
        "the radar must actually exercise running_membership's pull-in, or this test \
         cannot distinguish the two rules"
    );

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
// Geometry — popup vs full-screen
//
// The other half of T5, and the half the state tests above are blind to: a mount can
// satisfy every `Space`/`Esc` assertion here and still draw the popup wrong. These are
// pure `Rect` arithmetic (`focus_placement`), so they grade it without a pseudo-terminal;
// what the popup *contains* at each size is `s11_focus_plan.rs`'s job, and the painted
// pixels are the attended PTY suite's (`PLAN-focus-panel.md` §9).
//
// The switch points come from `PROPOSAL-focus-panel.md` §10's pressure table, which
// already decided them. Nothing new is invented here, and the exact popup dimensions are
// deliberately NOT pinned — only the properties §10 states: ≤80% of the terminal in both
// axes, centred, and full-screen once an overlay stops being worth it.
// ---------------------------------------------------------------------------

/// §10's terminal sizes that must still be an overlay.
const POPUP_SIZES: [(u16, u16); 4] = [(120, 40), (100, 30), (80, 24), (60, 20)];

/// §10's terminal sizes at which the popup "no longer fits" and `Space` renders the panel
/// full-screen instead of refusing. 24×6 is the panel floor itself; below it the mount
/// shows the too-small message, which is not this function's decision.
const FULLSCREEN_SIZES: [(u16, u16); 3] = [(48, 14), (36, 10), (24, 6)];

#[test]
fn the_pressure_table_sizes_that_must_be_a_popup() {
    for (w, h) in POPUP_SIZES {
        let placement = focus_placement(Rect::new(0, 0, w, h));
        assert!(
            matches!(placement, FocusPlacement::Popup(_)),
            "{w}x{h} is a popup row in PROPOSAL §10, got {placement:?}"
        );
    }
}

#[test]
fn the_pressure_table_sizes_that_must_fall_back_to_full_screen() {
    for (w, h) in FULLSCREEN_SIZES {
        assert_eq!(
            focus_placement(Rect::new(0, 0, w, h)),
            FocusPlacement::FullScreen,
            "{w}x{h} is a full-screen row in PROPOSAL §10"
        );
    }
}

#[test]
fn the_popup_never_exceeds_eighty_percent_of_the_terminal() {
    // The ratio *is* the rule (§10's closing note): past it, "an overlay on the
    // Dashboard" has stopped meaning anything.
    for (w, h) in POPUP_SIZES {
        let FocusPlacement::Popup(popup) = focus_placement(Rect::new(0, 0, w, h)) else {
            continue; // reported by the popup-rows test above
        };
        assert!(
            popup.width * 5 <= w * 4,
            "{w}x{h}: popup width {} exceeds 80% of {w}",
            popup.width
        );
        assert!(
            popup.height * 5 <= h * 4,
            "{w}x{h}: popup height {} exceeds 80% of {h}",
            popup.height
        );
    }
}

#[test]
fn the_popup_is_centred_and_inside_the_frame() {
    for (w, h) in POPUP_SIZES {
        let area = Rect::new(0, 0, w, h);
        let FocusPlacement::Popup(popup) = focus_placement(area) else {
            continue;
        };
        assert!(
            popup.right() <= area.right() && popup.bottom() <= area.bottom(),
            "{w}x{h}: popup {popup:?} escapes the frame"
        );
        // Centred to within a cell in each axis — an odd remainder has to land somewhere.
        let left_gap = popup.x - area.x;
        let right_gap = area.right() - popup.right();
        let top_gap = popup.y - area.y;
        let bottom_gap = area.bottom() - popup.bottom();
        assert!(
            left_gap.abs_diff(right_gap) <= 1,
            "{w}x{h}: popup not horizontally centred ({left_gap} vs {right_gap})"
        );
        assert!(
            top_gap.abs_diff(bottom_gap) <= 1,
            "{w}x{h}: popup not vertically centred ({top_gap} vs {bottom_gap})"
        );
    }
}

#[test]
fn the_popups_content_rect_clears_the_panel_floor() {
    // The popup's border is chrome the mount subtracts before calling `plan_rungs`
    // (that function's contract). A popup whose *inner* rect is below the panel floor is
    // exactly the case §10 says must become `FullScreen` instead.
    for (w, h) in POPUP_SIZES {
        let FocusPlacement::Popup(popup) = focus_placement(Rect::new(0, 0, w, h)) else {
            continue;
        };
        assert!(
            popup.width >= petri::focus::MIN_FOCUS_WIDTH + 2
                && popup.height >= petri::focus::MIN_FOCUS_HEIGHT + 2,
            "{w}x{h}: popup {popup:?} has no room for a panel inside its border"
        );
    }
}

#[test]
fn a_degenerate_frame_does_not_panic() {
    // `dashboard::render`'s own contract (0x0, 1x1) extended to the mount, since a resize
    // can hand us either mid-frame.
    for (w, h) in [(0, 0), (1, 1), (0, 40), (120, 0)] {
        let _ = focus_placement(Rect::new(0, 0, w, h));
    }
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
