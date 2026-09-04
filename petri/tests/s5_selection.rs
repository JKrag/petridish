//! S5 acceptance gate, layer 1 (petri/SPEC.md §3.1/§8, ADR-0003) — protected,
//! authored by the orchestrator, not the delegate. Pure-state tests over
//! `petri::browser::BrowserState` — no `Frame`, no terminal, the cheap
//! exhaustive layer. This is where the Browser's real logic bugs live (per
//! ADR-0003's own layer-1 description), and S6's Dashboard collapsed-section
//! navigation (the named trap in petri/SPEC.md §3.2) reuses this selection
//! machinery, so a weak gate here makes S6 more expensive later.
//!
//! `BrowserState::{new,move_selection,apply_filter,selected_project}` are all
//! `todo!()` stubs — every test below FAILS (panics) rather than errors,
//! confirmed before delegating S5.

use petri::browser::BrowserState;
use petridish_core::schema::Radar;
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn load(name: &str) -> Radar {
    let text = std::fs::read_to_string(fixture_path(name))
        .unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("fixture {name} failed to deserialize into Radar: {e}"))
}

#[test]
fn new_excludes_foreign_projects() {
    let radar = load("loaded.json");
    let state = BrowserState::new(&radar);
    for &idx in &state.visible {
        assert!(
            !radar.projects[idx].is_foreign,
            "visible must never include an is_foreign project, got index {idx} ({:?})",
            radar.projects[idx].name
        );
    }
}

#[test]
fn new_groups_in_section_order() {
    let radar = load("loaded.json");
    let state = BrowserState::new(&radar);
    // Section order is active, in_flight, stale, cold (petri/SPEC.md §3.1) —
    // once we've moved past a bucket in `visible`, we must never see an
    // earlier bucket again.
    use petridish_core::schema::StatusBucket;
    let rank = |b: StatusBucket| match b {
        StatusBucket::Active => 0,
        StatusBucket::InFlight => 1,
        StatusBucket::Stale => 2,
        StatusBucket::Cold => 3,
    };
    let mut last_rank = 0;
    for &idx in &state.visible {
        let r = rank(radar.projects[idx].status_bucket);
        assert!(
            r >= last_rank,
            "visible must be grouped active->in_flight->stale->cold in order, saw rank {r} after rank {last_rank}"
        );
        last_rank = r;
    }
}

#[test]
fn new_selects_first_visible_row() {
    let radar = load("minimal.json");
    let state = BrowserState::new(&radar);
    assert!(
        !state.visible.is_empty(),
        "minimal.json must have a visible project"
    );
    assert_eq!(
        state.selected,
        Some(0),
        "initial selection must be the first visible row"
    );
}

#[test]
fn empty_visible_list_has_no_selection_and_does_not_panic() {
    // hostile.json is all-cold but not empty (see petridish-core's
    // fixtures_test.rs for why) — build a genuinely empty Radar instead to
    // exercise the true empty-list case petri/SPEC.md §3.1 requires be
    // representable.
    let radar = Radar {
        schema_version: 1,
        updated_at: chrono::Utc::now(),
        scan_duration_ms: 0,
        projects: vec![],
        quota: None,
    };
    let mut state = BrowserState::new(&radar);
    assert_eq!(
        state.selected, None,
        "empty visible list must select None, not panic or default to Some(0)"
    );
    // Moving selection on an empty list must be a safe no-op.
    state.move_selection(1);
    state.move_selection(-1);
    assert_eq!(state.selected, None);
    assert!(state.selected_project(&radar).is_none());
}

#[test]
fn move_selection_clamps_at_the_bottom_never_wraps() {
    let radar = load("normal.json"); // 15 projects (petri/SPEC.md §8)
    let mut state = BrowserState::new(&radar);
    let last = state.visible.len() - 1;
    // Move far past the end — must clamp at the last row, not wrap to 0.
    state.move_selection(1000);
    assert_eq!(
        state.selected,
        Some(last),
        "must clamp at the last visible row, never wrap"
    );
}

#[test]
fn move_selection_clamps_at_the_top_never_wraps() {
    let radar = load("normal.json");
    let mut state = BrowserState::new(&radar);
    state.move_selection(1000); // go to the bottom first
    state.move_selection(-1000); // then far past the top
    assert_eq!(
        state.selected,
        Some(0),
        "must clamp at the first visible row, never wrap"
    );
}

#[test]
fn move_selection_by_i32_extremes_jumps_to_edges_without_overflow() {
    // `lib.rs` binds Home/End to `move_selection(i32::MIN)`/`(i32::MAX)` —
    // guards against the overflow-panic regression where `current + delta`
    // (plain `+`, not `saturating_add`) would panic in a debug build once
    // `current + i32::MAX` exceeded `i32::MAX`.
    let radar = load("normal.json");
    let mut state = BrowserState::new(&radar);
    let last = state.visible.len() - 1;

    state.move_selection(i32::MAX);
    assert_eq!(
        state.selected,
        Some(last),
        "i32::MAX must jump to the last row, not overflow/panic"
    );

    state.move_selection(i32::MIN);
    assert_eq!(
        state.selected,
        Some(0),
        "i32::MIN must jump to the first row, not overflow/panic"
    );
}

#[test]
fn move_selection_crosses_section_boundaries() {
    // loaded.json has every bucket populated (petri/SPEC.md §8) — moving one
    // step past the end of a section's rows must land in the next section's
    // first row, not stop at the boundary (the Browser has no headers to
    // stop on, unlike the Dashboard — petri/SPEC.md §3.2).
    let radar = load("loaded.json");
    let mut state = BrowserState::new(&radar);
    use petridish_core::schema::StatusBucket;
    let bucket_at =
        |state: &BrowserState, pos: usize| radar.projects[state.visible[pos]].status_bucket;

    let start_bucket = bucket_at(&state, 0);
    assert_eq!(
        start_bucket,
        StatusBucket::Active,
        "loaded.json's first visible row must be active"
    );

    // Walk forward one row at a time until the bucket changes; this must
    // happen well before the end of the list (loaded.json has ~70 projects
    // across 4 buckets), proving move_selection doesn't get stuck at a
    // section boundary.
    let mut crossed = false;
    for _ in 0..state.visible.len() {
        state.move_selection(1);
        let pos = state.selected.expect("still non-empty");
        if bucket_at(&state, pos) != start_bucket {
            crossed = true;
            break;
        }
    }
    assert!(
        crossed,
        "move_selection must cross from the active section into the next one"
    );
}

#[test]
fn apply_filter_case_insensitive_substring_match() {
    let radar = load("normal.json");
    let mut state = BrowserState::new(&radar);
    // Pick a real project name from the fixture and search a lowercase
    // substring of it in an uppercase form to prove case-insensitivity.
    let target_name = radar.projects[state.visible[0]].name.clone();
    let needle = target_name
        .chars()
        .take(3)
        .collect::<String>()
        .to_uppercase();
    state.apply_filter(&radar, &needle);
    assert!(
        !state.visible.is_empty(),
        "filtering by an uppercase substring of a real project name must still match it"
    );
    for &idx in &state.visible {
        assert!(
            radar.projects[idx]
                .name
                .to_lowercase()
                .contains(&needle.to_lowercase()),
            "every visible project after filtering must match the query"
        );
    }
}

#[test]
fn apply_filter_empty_query_returns_full_unfiltered_list() {
    let radar = load("normal.json");
    let mut state = BrowserState::new(&radar);
    let full = state.visible.clone();
    state.apply_filter(&radar, "something that matches nothing at all xyz123");
    assert!(
        state.visible.len() < full.len(),
        "sanity: the nonsense query must actually filter something out"
    );
    state.apply_filter(&radar, "");
    assert_eq!(
        state.visible, full,
        "an empty query must return the input (full grouped list) unchanged"
    );
}

#[test]
fn apply_filter_out_the_selected_project_resets_to_first_available_row() {
    let radar = load("normal.json");
    let mut state = BrowserState::new(&radar);
    // Select some project in the middle, then filter it out entirely.
    state.move_selection(3);
    let selected_name = radar.projects[state.visible[state.selected.unwrap()]]
        .name
        .clone();
    let disjoint_query = "zzz_definitely_not_a_real_project_name_zzz";
    assert!(
        !selected_name.to_lowercase().contains(disjoint_query),
        "sanity: query must not accidentally match the selected project's own name"
    );
    state.apply_filter(&radar, disjoint_query);
    // Query matches nothing at all -> visible is empty -> selection must be
    // None, not panic and not point at an out-of-bounds index.
    assert!(state.visible.is_empty());
    assert_eq!(
        state.selected, None,
        "filtering out everything (including the selection) must not panic and must clear selection"
    );
}

#[test]
fn apply_filter_keeps_selection_on_the_same_project_when_it_survives() {
    let radar = load("normal.json");
    let mut state = BrowserState::new(&radar);
    state.move_selection(2);
    let selected_idx_in_projects = state.visible[state.selected.unwrap()];
    let selected_name = radar.projects[selected_idx_in_projects].name.clone();
    // Filter by a substring of the selected project's own name — it survives.
    let needle: String = selected_name.chars().take(2).collect();
    state.apply_filter(&radar, &needle);
    let new_pos = state
        .selected
        .expect("selected project matches its own name substring, must stay selected");
    assert_eq!(
        state.visible[new_pos], selected_idx_in_projects,
        "selection must follow the same project across a filter that doesn't exclude it"
    );
}
