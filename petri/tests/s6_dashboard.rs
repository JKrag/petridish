//! S6 acceptance gate, layer 1 (petri/SPEC.md §3.2/§8, ADR-0003, ADR-0001) —
//! protected, authored by the orchestrator, not the delegate. Pure-state
//! contract tests for `DashboardState`, no terminal/rendering involved.
//!
//! This is the file that actually pins the named trap petri/SPEC.md §3.2
//! calls out explicitly: "Section headers are selection stops, and this is
//! load-bearing." A collapsed section renders zero rows, and `STALE`/`COLD`
//! ship collapsed by default — if the cursor only ever visited rows, there
//! would be no way to select a collapsed section and therefore no way to
//! reopen it. Every test below that exercises `collapsed`/`toggle_selected`
//! exists to make a regression of that specific trap fail loudly here,
//! before it can ship silently (a delegate reusing `BrowserState`'s flat
//! `Vec<usize>` model — which cannot represent a header stop at all — is the
//! anticipated failure mode; see `browser.rs`'s corrected doc comment on
//! `SECTION_ORDER`).
//!
//! Scope note (engineering-integrity disclosure, not a silent gap): the
//! compact-section ("IN FLIGHT"/"STALE"/"COLD") worktree rollup rule in
//! petri/SPEC.md §3.2 ("a parent with worktree children in any bucket shows
//! `name · N worktrees` on its own row rather than listing them") is
//! genuinely ambiguous for a parent whose OWN section is RUNNING while its
//! worktree child sits in a different (compact) section — the spec text
//! does not resolve where, if anywhere, that child's existence gets
//! reflected. The only worktree relationship across all four fixtures
//! (`delta-05-worktree` parented under `alpha-01`, in `tests/fixtures/loaded.json`)
//! is exactly this ambiguous case. Rather than pin an invented resolution,
//! this gate does NOT assert compact-section worktree-rollup rendering at
//! all — `running_membership`'s tests below cover the unambiguous ADR-0001
//! RUNNING-membership rule only. See `.afk/prompt-S6.md` for the note asking
//! the delegate to make a documented best-effort call.

use chrono::{Duration as ChronoDuration, Utc};
use petri::dashboard::{DashRow, DashboardState, SECTION_ORDER};
use petridish_core::schema::{AgentState, GitState, Project, Radar, StatusBucket};
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
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("fixture {name} failed to deserialize into Radar: {e}"))
}

/// A minimal, otherwise-default project — callers override only the fields
/// they care about. Keeps the hand-built-`Radar` tests below focused on the
/// one or two fields each is actually testing.
fn project(id: &str, name: &str, bucket: StatusBucket) -> Project {
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
        status_bucket: bucket,
    }
}

fn radar_of(projects: Vec<Project>) -> Radar {
    Radar {
        schema_version: 1,
        updated_at: Utc::now(),
        scan_duration_ms: 0,
        projects,
        quota: None,
    }
}

fn section_index(b: StatusBucket) -> usize {
    SECTION_ORDER.iter().position(|s| *s == b).expect("bucket must be in SECTION_ORDER")
}

// --- Default collapse state and initial cursor -----------------------------

#[test]
fn new_defaults_running_and_in_flight_expanded_stale_and_cold_collapsed() {
    let radar = load("loaded.json"); // 25 active / 18 in_flight / 15 stale / 12 cold
    let state = DashboardState::new(&radar);
    assert_eq!(
        state.collapsed,
        [false, false, true, true],
        "petri/SPEC.md §3.2 defaults: RUNNING and IN FLIGHT expanded, STALE and COLD collapsed"
    );
}

#[test]
fn new_selects_the_first_stop_which_is_the_running_header() {
    let radar = load("loaded.json");
    let state = DashboardState::new(&radar);
    assert_eq!(state.selected, Some(0));
    assert_eq!(
        state.visible.first().copied(),
        Some(DashRow::Header(StatusBucket::Active)),
        "the first cursor stop must be RUNNING's header, not a project row"
    );
}

// --- The named trap: collapsed vs. empty ------------------------------------

#[test]
fn collapsed_sections_are_a_stop_but_contribute_zero_row_stops() {
    let radar = load("loaded.json"); // STALE (15) and COLD (12) both non-empty, both collapsed by default
    let state = DashboardState::new(&radar);

    let stale_headers = state.visible.iter().filter(|r| **r == DashRow::Header(StatusBucket::Stale)).count();
    let cold_headers = state.visible.iter().filter(|r| **r == DashRow::Header(StatusBucket::Cold)).count();
    assert_eq!(stale_headers, 1, "a non-empty collapsed section still gets exactly one header stop");
    assert_eq!(cold_headers, 1, "a non-empty collapsed section still gets exactly one header stop");

    for row in &state.visible {
        if let DashRow::Project(idx) = row {
            let bucket = &radar.projects[*idx].status_bucket;
            assert!(
                !matches!(bucket, StatusBucket::Stale | StatusBucket::Cold),
                "collapsed sections must contribute zero row stops — found a {bucket:?} project row while STALE/COLD are collapsed"
            );
        }
    }
}

#[test]
fn empty_section_contributes_no_stop_at_all_not_even_a_header() {
    let radar = load("hostile.json"); // all 7 projects are cold; active/in_flight/stale are empty and have no worktree relationships
    let state = DashboardState::new(&radar);

    for bucket in [StatusBucket::Active, StatusBucket::InFlight, StatusBucket::Stale] {
        assert!(
            !state.visible.contains(&DashRow::Header(bucket)),
            "an empty section (no projects, per hostile.json's all-cold shape) must not contribute a header stop for {bucket:?}"
        );
    }
    // COLD is the only non-empty section, and it's collapsed by default —
    // so the entire cursor sequence is exactly one stop: COLD's header.
    assert_eq!(state.visible, vec![DashRow::Header(StatusBucket::Cold)]);
    assert_eq!(state.selected, Some(0));
}

// --- Space/toggle semantics --------------------------------------------------

#[test]
fn toggling_a_header_reveals_or_hides_that_sections_rows() {
    let radar = load("loaded.json");
    let mut state = DashboardState::new(&radar);

    let stale_header_pos = state
        .visible
        .iter()
        .position(|r| *r == DashRow::Header(StatusBucket::Stale))
        .expect("STALE header must be a stop even while collapsed");
    state.selected = Some(stale_header_pos);

    state.toggle_selected(&radar);
    assert!(!state.collapsed[section_index(StatusBucket::Stale)], "toggling STALE's header must expand it");
    let stale_rows_after_expand = state
        .visible
        .iter()
        .filter(|r| matches!(r, DashRow::Project(idx) if radar.projects[*idx].status_bucket == StatusBucket::Stale))
        .count();
    assert_eq!(stale_rows_after_expand, 15, "expanding STALE must reveal all 15 of its project rows");

    // Toggle again: back to collapsed, zero rows.
    let stale_header_pos_2 = state
        .visible
        .iter()
        .position(|r| *r == DashRow::Header(StatusBucket::Stale))
        .expect("STALE header must still be a stop after expanding");
    state.selected = Some(stale_header_pos_2);
    state.toggle_selected(&radar);
    assert!(state.collapsed[section_index(StatusBucket::Stale)], "toggling STALE's header again must re-collapse it");
}

#[test]
fn toggling_a_row_collapses_its_section_and_moves_selection_to_that_header() {
    let radar = load("loaded.json"); // RUNNING is expanded by default and non-empty
    let mut state = DashboardState::new(&radar);

    let running_row_pos = state
        .visible
        .iter()
        .position(|r| matches!(r, DashRow::Project(idx) if radar.projects[*idx].status_bucket == StatusBucket::Active))
        .expect("RUNNING must have at least one visible row by default");
    state.selected = Some(running_row_pos);

    state.toggle_selected(&radar);

    assert!(state.collapsed[section_index(StatusBucket::Active)], "toggling a RUNNING row must collapse RUNNING");
    let selected_row = state.visible.get(state.selected.expect("selection must not become None"));
    assert_eq!(
        selected_row.copied(),
        Some(DashRow::Header(StatusBucket::Active)),
        "the cursor must never be left pointing at a row that just stopped existing — it must land on that section's header"
    );
}

// --- Clamping ----------------------------------------------------------------

#[test]
fn move_selection_clamps_at_both_ends_never_wraps() {
    let radar = load("loaded.json");
    let mut state = DashboardState::new(&radar);
    let len = state.visible.len();

    state.move_selection(-1000);
    assert_eq!(state.selected, Some(0));

    state.move_selection(1000);
    assert_eq!(state.selected, Some(len - 1));
}

#[test]
fn move_selection_and_toggle_are_no_ops_on_an_empty_dashboard() {
    let radar = radar_of(vec![]);
    let mut state = DashboardState::new(&radar);
    assert!(state.visible.is_empty());
    assert_eq!(state.selected, None);

    state.move_selection(1);
    assert_eq!(state.selected, None);
    state.toggle_selected(&radar); // must not panic
    assert_eq!(state.selected, None);
}

// --- ADR-0001 RUNNING membership + quietest-first ordering -------------------

#[test]
fn running_membership_includes_active_bucket_projects_and_excludes_foreign() {
    let mut foreign_active = project("f1", "foreign-active", StatusBucket::Active);
    foreign_active.is_foreign = true;
    let plain_active = project("p1", "plain-active", StatusBucket::Active);
    let plain_cold = project("p2", "plain-cold", StatusBucket::Cold);

    let radar = radar_of(vec![foreign_active, plain_active, plain_cold]);
    let membership = DashboardState::running_membership(&radar);

    assert_eq!(membership, vec![1], "only the non-foreign Active project (index 1) belongs in RUNNING");
}

#[test]
fn running_membership_includes_a_non_active_parent_with_an_active_worktree_child() {
    // ADR-0001: "parent is active if a worktree child is active" — display-only
    // rollup into RUNNING membership, independent of the parent's own status_bucket.
    let mut parent = project("parent", "quiet-parent", StatusBucket::Cold);
    parent.path = "/repos/quiet-parent".to_string();
    let mut child = project("child", "child-worktree", StatusBucket::Active);
    child.parent_path = Some(parent.path.clone());

    let unrelated = project("u1", "unrelated-cold", StatusBucket::Cold);

    let radar = radar_of(vec![parent, child, unrelated]);
    let membership = DashboardState::running_membership(&radar);

    assert!(membership.contains(&0), "the cold parent must be pulled into RUNNING by its active worktree child");
    assert!(membership.contains(&1), "the active worktree child is itself Active-bucket and belongs in RUNNING on its own merit");
    assert!(!membership.contains(&2), "an unrelated cold project with no active worktree child stays out of RUNNING");
}

#[test]
fn running_membership_does_not_pull_in_a_parent_whose_worktree_child_is_not_active() {
    let mut parent = project("parent", "alpha-01", StatusBucket::Cold);
    parent.path = "/repos/alpha-01".to_string();
    let mut child = project("child", "delta-05-worktree", StatusBucket::Cold);
    child.parent_path = Some(parent.path.clone());

    let radar = radar_of(vec![parent, child]);
    let membership = DashboardState::running_membership(&radar);

    assert!(membership.is_empty(), "a cold parent with only a cold (non-active) worktree child has no RUNNING membership at all");
}

#[test]
fn running_membership_orders_quietest_first_within_the_attention_ceiling_then_the_forgotten_group() {
    // Superseded by real-world use (see `RUNNING_ATTENTION_CEILING_S`'s doc
    // comment in dashboard.rs): unbounded quietest-first let a project idle
    // for days permanently outrank one actively prompted minutes ago. Now
    // "quietest first" only competes within `RUNNING_ATTENTION_CEILING_S`
    // (3h); everything past it — silence that means "probably forgotten,"
    // not "might be stalled" — sorts as one group below, quietest-first
    // internally too, but never above the still-fresh group.
    let now = Utc::now();
    let mut fresh_active = project("f1", "fresh-active", StatusBucket::Active);
    fresh_active.last_activity_at = Some(now - ChronoDuration::minutes(5));
    let mut long_silent = project("s1", "long-silent", StatusBucket::Active);
    long_silent.last_activity_at = Some(now - ChronoDuration::hours(10));
    let never_seen = project("n1", "never-seen", StatusBucket::Active); // last_activity_at: None
    let mut recently_active = project("r1", "recently-active", StatusBucket::Active);
    recently_active.last_activity_at = Some(now - ChronoDuration::hours(1));

    // Deliberately inserted out of order to prove `running_membership` sorts
    // rather than preserving input order.
    let radar = radar_of(vec![fresh_active, long_silent, never_seen, recently_active]);
    let membership = DashboardState::running_membership(&radar);

    assert_eq!(
        membership,
        vec![3, 0, 2, 1],
        "within the 3h ceiling, quietest first: 1h-silent (index 3) before 5m-silent (index 0); \
         past the ceiling, quietest first but demoted as a group: never-seen/None (index 2, \
         maximally silent) before 10h-silent (index 1) — the forgotten group never outranks the \
         fresh one, but keeps quietest-first ordering internally"
    );
}
