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
use petri::dashboard::{DashRow, DashboardState, SECTION_ORDER, SelectionAnchor};
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
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("fixture {name} failed to deserialize into Radar: {e}"))
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
        agent_activity: Vec::new(),
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
    SECTION_ORDER
        .iter()
        .position(|s| *s == b)
        .expect("bucket must be in SECTION_ORDER")
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

    let stale_headers = state
        .visible
        .iter()
        .filter(|r| **r == DashRow::Header(StatusBucket::Stale))
        .count();
    let cold_headers = state
        .visible
        .iter()
        .filter(|r| **r == DashRow::Header(StatusBucket::Cold))
        .count();
    assert_eq!(
        stale_headers, 1,
        "a non-empty collapsed section still gets exactly one header stop"
    );
    assert_eq!(
        cold_headers, 1,
        "a non-empty collapsed section still gets exactly one header stop"
    );

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

    for bucket in [
        StatusBucket::Active,
        StatusBucket::InFlight,
        StatusBucket::Stale,
    ] {
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
    assert!(
        !state.collapsed[section_index(StatusBucket::Stale)],
        "toggling STALE's header must expand it"
    );
    let stale_rows_after_expand = state
        .visible
        .iter()
        .filter(|r| matches!(r, DashRow::Project(idx) if radar.projects[*idx].status_bucket == StatusBucket::Stale))
        .count();
    assert_eq!(
        stale_rows_after_expand, 15,
        "expanding STALE must reveal all 15 of its project rows"
    );

    // Toggle again: back to collapsed, zero rows.
    let stale_header_pos_2 = state
        .visible
        .iter()
        .position(|r| *r == DashRow::Header(StatusBucket::Stale))
        .expect("STALE header must still be a stop after expanding");
    state.selected = Some(stale_header_pos_2);
    state.toggle_selected(&radar);
    assert!(
        state.collapsed[section_index(StatusBucket::Stale)],
        "toggling STALE's header again must re-collapse it"
    );
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

    assert!(
        state.collapsed[section_index(StatusBucket::Active)],
        "toggling a RUNNING row must collapse RUNNING"
    );
    let selected_row = state
        .visible
        .get(state.selected.expect("selection must not become None"));
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

    assert_eq!(
        membership,
        vec![1],
        "only the non-foreign Active project (index 1) belongs in RUNNING"
    );
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

    assert!(
        membership.contains(&0),
        "the cold parent must be pulled into RUNNING by its active worktree child"
    );
    assert!(
        membership.contains(&1),
        "the active worktree child is itself Active-bucket and belongs in RUNNING on its own merit"
    );
    assert!(
        !membership.contains(&2),
        "an unrelated cold project with no active worktree child stays out of RUNNING"
    );
}

#[test]
fn running_membership_does_not_pull_in_a_parent_whose_worktree_child_is_not_active() {
    let mut parent = project("parent", "alpha-01", StatusBucket::Cold);
    parent.path = "/repos/alpha-01".to_string();
    let mut child = project("child", "delta-05-worktree", StatusBucket::Cold);
    child.parent_path = Some(parent.path.clone());

    let radar = radar_of(vec![parent, child]);
    let membership = DashboardState::running_membership(&radar);

    assert!(
        membership.is_empty(),
        "a cold parent with only a cold (non-active) worktree child has no RUNNING membership at all"
    );
}

#[test]
fn running_membership_orders_quietest_first_within_the_attention_ceiling_then_the_forgotten_group()
{
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

#[test]
fn running_membership_sorts_a_waiting_project_above_everything_including_the_fresh_group() {
    // MECH-5. The waiting project is deliberately given the *most* favourable-to-the-old-rule
    // shape possible: it is 10 hours silent, so under the previous ordering it would sit in
    // the forgotten group at the very bottom. That is exactly the inversion the feature
    // exists to fix — the ceiling demotes runs nobody is coming back to, and a pending
    // permission prompt is the one kind of silence with a known cause.
    let now = Utc::now();
    let mut fresh_active = project("f1", "fresh-active", StatusBucket::Active);
    fresh_active.last_activity_at = Some(now - ChronoDuration::minutes(5));
    let mut quiet_but_fresh = project("q1", "quiet-but-fresh", StatusBucket::Active);
    quiet_but_fresh.last_activity_at = Some(now - ChronoDuration::hours(1));
    let mut waiting = project("w1", "waiting-on-you", StatusBucket::Active);
    waiting.last_activity_at = Some(now - ChronoDuration::hours(10));
    waiting.agent.waiting_since = Some(now - ChronoDuration::minutes(20));

    let radar = radar_of(vec![fresh_active, quiet_but_fresh, waiting]);
    let membership = DashboardState::running_membership(&radar);

    assert_eq!(
        membership,
        vec![2, 1, 0],
        "the waiting project (index 2) leads despite being the most silent of the three; the \
         rest keep quietest-first inside the ceiling"
    );
}

#[test]
fn running_membership_ignores_an_expired_waiting_latch() {
    // The two-clocks rule: the scanner releases an expired latch on its next tick, but petri
    // may be drawing a `projects.json` written before that tick — or, if the daemon died,
    // hours before it. An expired latch must not keep a project pinned to the top.
    let now = Utc::now();
    let mut fresh_active = project("f1", "fresh-active", StatusBucket::Active);
    fresh_active.last_activity_at = Some(now - ChronoDuration::minutes(5));
    let mut stale_latch = project("w1", "stale-latch", StatusBucket::Active);
    stale_latch.last_activity_at = Some(now - ChronoDuration::minutes(2));
    stale_latch.agent.waiting_since =
        Some(now - ChronoDuration::seconds(petridish_core::schema::WAITING_MAX_LATCH_S + 60));

    let radar = radar_of(vec![fresh_active, stale_latch]);
    let membership = DashboardState::running_membership(&radar);

    assert_eq!(
        membership,
        vec![0, 1],
        "with the latch expired, plain quietest-first applies: 5m-silent before 2m-silent"
    );
}

#[test]
fn two_waiting_projects_keep_quietest_first_among_themselves() {
    // The waiting key groups; it does not flatten the ordering inside the group.
    let now = Utc::now();
    let mut recent_wait = project("w1", "recent-wait", StatusBucket::Active);
    recent_wait.last_activity_at = Some(now - ChronoDuration::minutes(2));
    recent_wait.agent.waiting_since = Some(now - ChronoDuration::minutes(2));
    let mut older_wait = project("w2", "older-wait", StatusBucket::Active);
    older_wait.last_activity_at = Some(now - ChronoDuration::minutes(40));
    older_wait.agent.waiting_since = Some(now - ChronoDuration::minutes(40));
    let mut fresh_active = project("f1", "fresh-active", StatusBucket::Active);
    fresh_active.last_activity_at = Some(now - ChronoDuration::minutes(9));

    let radar = radar_of(vec![recent_wait, older_wait, fresh_active]);

    assert_eq!(
        DashboardState::running_membership(&radar),
        vec![1, 0, 2],
        "both waiting projects lead, quietest-first between them; the non-waiting one follows"
    );
}

// --- Reload preserves user state (regression) ------------------------------
//
// The Dashboard re-derives itself every time `swab` rewrites the state file,
// which on an active machine is every few seconds. It used to do that with
// `DashboardState::new`, which hardcodes the spec defaults — so a section the
// user had collapsed reopened itself, and the cursor jumped back to the top,
// with no input from the user. These pin the two halves of that fix.
//
// Collapse is the load-bearing half: petri/SPEC.md §3.2 calls collapse the way
// real estate is allocated on this screen "deliberately by the user, not by a
// fixed priority ladder", which a reload that overrides it makes false.

#[test]
fn refresh_preserves_a_collapsed_section_across_a_reload() {
    let radar = load("loaded.json");

    // Collapse IN FLIGHT, the case actually reported.
    let in_flight = section_index(StatusBucket::InFlight);
    let mut collapsed = [false, false, true, true];
    collapsed[in_flight] = true;
    let mut state = DashboardState::with_collapsed(&radar, collapsed);
    let rows_while_collapsed = state.visible.len();

    // A fresh scan lands: same shape, new Radar value.
    let reloaded = load("loaded.json");
    state.refresh(&reloaded, None);

    assert!(
        state.collapsed[in_flight],
        "a reload must not reopen a section the user collapsed"
    );
    assert_eq!(
        state.visible.len(),
        rows_while_collapsed,
        "row count must not change across a reload that changed nothing else"
    );
}

#[test]
fn refresh_keeps_every_section_collapse_flag_not_just_one() {
    let radar = load("loaded.json");
    // The inverse of the defaults, so a reset to defaults cannot pass by accident.
    let mut state = DashboardState::with_collapsed(&radar, [true, true, false, false]);

    state.refresh(&load("loaded.json"), None);

    assert_eq!(state.collapsed, [true, true, false, false]);
}

#[test]
fn refresh_restores_the_cursor_to_the_same_project() {
    let radar = load("loaded.json");
    let mut state = DashboardState::new(&radar);

    // Walk onto a project row (not a header) and record what it points at.
    let mut steps = 0;
    while !matches!(state.visible[state.selected.unwrap()], DashRow::Project(_)) {
        state.move_selection(1);
        steps += 1;
        assert!(steps < 50, "fixture must contain a selectable project row");
    }
    let anchor = state.selection_anchor(&radar).expect("a row is selected");
    let name_before = match state.visible[state.selected.unwrap()] {
        DashRow::Project(i) => radar.projects[i].name.clone(),
        DashRow::Header(_) => unreachable!(),
    };

    state.refresh(&load("loaded.json"), Some(anchor));

    let name_after = match state.visible[state.selected.unwrap()] {
        DashRow::Project(i) => radar.projects[i].name.clone(),
        DashRow::Header(_) => panic!("cursor moved off the project row onto a header"),
    };
    assert_eq!(
        name_after, name_before,
        "the cursor must land on the same project, not the same index"
    );
}

#[test]
fn refresh_restores_the_cursor_to_the_same_header() {
    let radar = load("loaded.json");
    let mut state = DashboardState::new(&radar);
    state.move_selection(0); // sits on the first header
    let anchor = state
        .selection_anchor(&radar)
        .expect("a header is selected");
    assert!(matches!(anchor, SelectionAnchor::Header(_)));

    state.refresh(&load("loaded.json"), Some(anchor.clone()));

    assert_eq!(state.selection_anchor(&radar), Some(anchor));
}

#[test]
fn refresh_follows_the_project_when_the_scan_reorders_it() {
    // The whole reason the anchor carries an identity and not an index: `swab`
    // re-sorts on every tick, so the same index is a different project.
    let before = radar_of(vec![
        project("a", "alpha", StatusBucket::Active),
        project("b", "bravo", StatusBucket::Active),
    ]);
    let mut state = DashboardState::new(&before);
    state.move_selection(2); // header, alpha, bravo -> lands on bravo
    let anchor = state.selection_anchor(&before);
    assert_eq!(anchor, Some(SelectionAnchor::Project("b".into())));

    // Same two projects, opposite order.
    let after = radar_of(vec![
        project("b", "bravo", StatusBucket::Active),
        project("a", "alpha", StatusBucket::Active),
    ]);
    state.refresh(&after, anchor);

    assert_eq!(
        state.selection_anchor(&after),
        Some(SelectionAnchor::Project("b".into())),
        "the cursor must follow the project, not stay on the index it used to occupy"
    );
}

// Names are not unique — two checkouts under different roots are routinely called the
// same thing (the fleet this was built against has `smoke` three times). The anchor
// carried the display name first, which meant a reload restored the cursor to *a*
// project with that name rather than the one it was on: a silent wrong-target, which is
// worse than losing the cursor because it looks like it worked. Found in review.
#[test]
fn refresh_distinguishes_two_projects_that_share_a_name() {
    let before = radar_of(vec![
        project("work-smoke", "smoke", StatusBucket::Active),
        project("play-smoke", "smoke", StatusBucket::Active),
    ]);
    let mut state = DashboardState::new(&before);
    state.move_selection(2); // header, first smoke, second smoke -> the second
    let anchor = state.selection_anchor(&before);
    assert_eq!(
        anchor,
        Some(SelectionAnchor::Project("play-smoke".into())),
        "the anchor must carry the stable id, not the shared display name"
    );

    // Next scan puts the other one first — under a name anchor, the cursor would
    // "restore" onto work-smoke and nothing would look wrong.
    let after = radar_of(vec![
        project("play-smoke", "smoke", StatusBucket::Active),
        project("work-smoke", "smoke", StatusBucket::Active),
    ]);
    state.refresh(&after, anchor);

    let selected = state.selected.expect("something must stay selected");
    match state.visible[selected] {
        DashRow::Project(i) => assert_eq!(
            after.projects[i].id, "play-smoke",
            "the cursor must land on the same project, not the first one sharing its name"
        ),
        DashRow::Header(_) => panic!("cursor moved off the project row"),
    }
}

#[test]
fn refresh_clamps_rather_than_resetting_when_the_anchored_project_vanishes() {
    let before = radar_of(vec![
        project("a", "alpha", StatusBucket::Active),
        project("b", "bravo", StatusBucket::Active),
        project("c", "charlie", StatusBucket::Active),
    ]);
    let mut state = DashboardState::new(&before);
    state.move_selection(3); // header + 3 rows -> the last one, charlie
    let anchor = state.selection_anchor(&before);
    assert_eq!(anchor, Some(SelectionAnchor::Project("c".into())));

    // charlie is gone from the next scan.
    let after = radar_of(vec![
        project("a", "alpha", StatusBucket::Active),
        project("b", "bravo", StatusBucket::Active),
    ]);
    state.refresh(&after, anchor);

    let selected = state.selected.expect("something must stay selected");
    assert!(
        selected < state.visible.len(),
        "selection must stay in bounds after the list shrank"
    );
    assert_ne!(
        selected, 0,
        "a vanished project must clamp the cursor near where it was, not reset it to the top"
    );
}

#[test]
fn refresh_on_an_empty_radar_clears_the_selection_without_panicking() {
    let before = radar_of(vec![project("a", "alpha", StatusBucket::Active)]);
    let mut state = DashboardState::new(&before);
    let anchor = state.selection_anchor(&before);

    state.refresh(&radar_of(vec![]), anchor);

    assert!(state.visible.is_empty());
    assert_eq!(
        state.selected, None,
        "the empty selection must be representable"
    );
}
