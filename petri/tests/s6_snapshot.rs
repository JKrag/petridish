//! S6 acceptance gate, layer 2 (petri/SPEC.md §3.2/§8, ADR-0003) — protected,
//! authored by the orchestrator, not the delegate. Same structural-not-exact
//! approach as S4/S5's snapshot files (see `s4_snapshot.rs`'s module doc
//! comment for why).
//!
//! `dashboard::render`'s `todo!()` body means every test below FAILS (panics)
//! rather than errors — confirmed before delegating S6.
//!
//! Scope note: this file does not assert the compact-section worktree-rollup
//! rendering (`name · N worktrees`) — see `s6_dashboard.rs`'s module doc
//! comment for why that specific rule is ambiguous against the only fixture
//! that exercises it, and is deliberately left ungated here.
//!
//! Banner wording note: petri/SPEC.md §3.2 requires a staleness banner but
//! does not pin its exact text. `.afk/prompt-S6.md` asks the delegate to
//! include the literal substring "stale" (any case) referring to data
//! freshness — chosen because `hostile.json` (the fixture used to test this)
//! has zero STALE-bucket projects, so there is no risk of the section label
//! colliding with the banner check.

use petri::dashboard::DashboardState;
use petridish_core::schema::{AgentState, GitState, Project, Radar, StatusBucket};
use ratatui::{Terminal, backend::TestBackend};
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
        updated_at: chrono::Utc::now(),
        scan_duration_ms: 0,
        projects,
        quota: None,
    }
}

fn rendered_lines(radar: &Radar, state: &DashboardState, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal must construct");
    terminal
        .draw(|frame| petri::dashboard::render(frame, radar, state))
        .expect("draw must not error");
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect::<String>()
        })
        .collect()
}

#[test]
fn header_identifies_the_dashboard_screen_at_80x24() {
    let radar = load("loaded.json");
    let state = DashboardState::new(&radar);
    let lines = rendered_lines(&radar, &state, 80, 24);
    assert!(lines[0].contains("petri"), "row 0 must contain \"petri\", got: {:?}", lines[0]);
    assert!(
        lines[0].to_lowercase().contains("dashboard"),
        "row 0 must identify this as the Dashboard screen (distinct from the Browser's \"petri · browser\"), got: {:?}",
        lines[0]
    );
}

#[test]
fn running_label_rendered_for_loaded_json_which_has_agents_present() {
    let radar = load("loaded.json"); // 25 active projects, most with an active_agent set
    let state = DashboardState::new(&radar);
    let lines = rendered_lines(&radar, &state, 80, 40);
    let whole = lines.join("\n");
    assert!(whole.contains("RUNNING"), "loaded.json's RUNNING section has agents present, so it must not degrade to RECENT, got:\n{whole}");
}

#[test]
fn running_label_degrades_to_recent_when_no_project_has_an_active_agent() {
    // petri/SPEC.md §3.2: "Label degrades to RECENT when nothing in the
    // section has an agent at all, because RUNNING would then overstate it."
    // Interpretation (documented, since the spec doesn't spell out the exact
    // condition): "has an agent at all" means at least one project in the
    // section has `agent.active_agent.is_some()`. Both projects here have
    // `active_agent: None` (via `AgentState::idle_unknown()`), so the
    // section must degrade.
    let p1 = project("p1", "idle-one", StatusBucket::Active);
    let p2 = project("p2", "idle-two", StatusBucket::Active);
    let radar = radar_of(vec![p1, p2]);
    let state = DashboardState::new(&radar);
    let lines = rendered_lines(&radar, &state, 80, 24);
    let whole = lines.join("\n");
    assert!(whole.contains("RECENT"), "no project has an active_agent, so the label must degrade to RECENT, got:\n{whole}");
    assert!(!whole.contains("RUNNING"), "must not show RUNNING when every project in the section is agent-less, got:\n{whole}");
}

#[test]
fn collapsed_sections_show_their_header_but_hide_their_project_names() {
    // Height bumped from 80 to 160: this test's intent (collapsed sections
    // hide their names) is unchanged, but the RUNNING/IN FLIGHT sections'
    // real row footprint grew once roomy cards became genuinely roomy
    // (3 content lines + a blank separator per project, not 1 line) — at 80
    // rows the render would truncate mid-RUNNING and never reach the STALE/
    // COLD headers this test checks for, for reasons unrelated to what it's
    // actually testing.
    let radar = load("loaded.json"); // STALE = charlie-*, COLD = delta-*, both collapsed by default
    let state = DashboardState::new(&radar);
    let lines = rendered_lines(&radar, &state, 200, 160);
    let whole = lines.join("\n");

    assert!(whole.contains("STALE"), "STALE's header must render even while collapsed, got:\n{whole}");
    assert!(whole.contains("COLD"), "COLD's header must render even while collapsed, got:\n{whole}");
    for name in ["charlie-01", "charlie-02"] {
        assert!(!whole.contains(name), "STALE is collapsed by default — {name:?} must not be visible, got:\n{whole}");
    }
    for name in ["delta-01", "delta-02"] {
        assert!(!whole.contains(name), "COLD is collapsed by default — {name:?} must not be visible, got:\n{whole}");
    }
}

#[test]
fn overflow_truncates_with_a_more_marker_instead_of_scrolling() {
    // petri/SPEC.md §3.2: "Overflow: truncate, do not scroll... The `… +N
    // more` marker is required." loaded.json's RUNNING (25) + IN FLIGHT (18)
    // alone vastly exceed a 15-row-tall terminal even before STALE/COLD
    // (collapsed by default) are considered.
    let radar = load("loaded.json");
    let state = DashboardState::new(&radar);
    let lines = rendered_lines(&radar, &state, 80, 15);
    let whole = lines.join("\n");
    assert!(
        whole.to_lowercase().contains("more"),
        "exceeding the terminal height must show a required \"+N more\" truncation marker, got:\n{whole}"
    );
}

#[test]
fn staleness_banner_rendered_when_updated_at_is_older_than_24h() {
    let radar = load("hostile.json"); // updated_at ~6 days old, and has zero STALE-bucket projects (see module doc comment)
    let state = DashboardState::new(&radar);
    let lines = rendered_lines(&radar, &state, 80, 24);
    let whole = lines.join("\n").to_lowercase();
    assert!(
        whole.contains("stale"),
        "updated_at older than 24h must render a persistent staleness banner (must contain the word \"stale\", any case — see module doc comment), got:\n{whole}"
    );
}

#[test]
fn does_not_panic_on_empty_radar() {
    let radar = radar_of(vec![]);
    let state = DashboardState::new(&radar);
    let _ = rendered_lines(&radar, &state, 80, 24);
}

#[test]
fn does_not_panic_at_tiny_geometry() {
    let radar = load("minimal.json");
    let state = DashboardState::new(&radar);
    let _ = rendered_lines(&radar, &state, 1, 1);
}
