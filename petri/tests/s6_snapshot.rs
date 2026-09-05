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
        .join("fixtures")
        .join(name)
}

fn load(name: &str) -> Radar {
    let text = std::fs::read_to_string(fixture_path(name))
        .unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("fixture {name} failed to deserialize into Radar: {e}"))
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
        .draw(|frame| {
            // SPACE-1 added a feed parameter. These S6 tests are about section layout, so
            // they pass an empty feed: `feed_rows_for` still grants rows on a tall enough
            // terminal, and `feed_block_lines` draws its "nothing yet" body — neither of
            // which any assertion here looks at.
            petri::dashboard::render(frame, radar, state, &petri::feed::FeedState::default())
        })
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
    assert!(
        lines[0].contains("petri"),
        "row 0 must contain \"petri\", got: {:?}",
        lines[0]
    );
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
    assert!(
        whole.contains("RUNNING"),
        "loaded.json's RUNNING section has agents present, so it must not degrade to RECENT, got:\n{whole}"
    );
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
    assert!(
        whole.contains("RECENT"),
        "no project has an active_agent, so the label must degrade to RECENT, got:\n{whole}"
    );
    assert!(
        !whole.contains("RUNNING"),
        "must not show RUNNING when every project in the section is agent-less, got:\n{whole}"
    );
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

    assert!(
        whole.contains("STALE"),
        "STALE's header must render even while collapsed, got:\n{whole}"
    );
    assert!(
        whole.contains("COLD"),
        "COLD's header must render even while collapsed, got:\n{whole}"
    );
    for name in ["charlie-01", "charlie-02"] {
        assert!(
            !whole.contains(name),
            "STALE is collapsed by default — {name:?} must not be visible, got:\n{whole}"
        );
    }
    for name in ["delta-01", "delta-02"] {
        assert!(
            !whole.contains(name),
            "COLD is collapsed by default — {name:?} must not be visible, got:\n{whole}"
        );
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

#[test]
fn roomy_running_card_renders_a_sparkline_from_agent_activity() {
    let mut p = project("p1", "spark-project", StatusBucket::Active);
    p.agent.active_agent = Some("claude-code".to_string());
    p.agent_activity = vec![0, 0, 5, 0, 3];
    let radar = radar_of(vec![p]);
    let state = DashboardState::new(&radar);
    // Tall enough that RUNNING renders in the roomy tier, not the compact one
    // (`COMPACT_TIER_MAX_CONTENT_ROWS` — see `dashboard.rs`).
    let lines = rendered_lines(&radar, &state, 100, 40);
    let whole = lines.join("\n");
    assert!(
        whole.contains('▁'),
        "the sparkline's zero/pad-level bar (U+2581) must render, got:\n{whole}"
    );
    assert!(
        whole
            .chars()
            .any(|c| ('\u{2582}'..='\u{2588}').contains(&c)),
        "at least one non-zero-level bar must render for the nonzero samples, got:\n{whole}"
    );
}

#[test]
fn compact_running_row_does_not_render_a_sparkline() {
    // Per the handoff's design: the sparkline is a roomy-tier-only enrichment. Force the
    // compact tier via a short terminal (below COMPACT_TIER_MAX_CONTENT_ROWS worth of
    // content rows) and confirm no sparkline glyph appears.
    let mut p = project("p1", "spark-project", StatusBucket::Active);
    p.agent.active_agent = Some("claude-code".to_string());
    p.agent_activity = vec![9; 20]; // would otherwise definitely render non-lowest bars
    let radar = radar_of(vec![p]);
    let state = DashboardState::new(&radar);
    let lines = rendered_lines(&radar, &state, 100, 12);
    let whole = lines.join("\n");
    assert!(
        !whole
            .chars()
            .any(|c| ('\u{2581}'..='\u{2588}').contains(&c)),
        "the compact tier must not render any sparkline glyph, got:\n{whole}"
    );
}

#[test]
fn roomy_running_card_renders_a_git_zone_row_with_its_own_sparkline() {
    // Redesign (dashboard card layout review): the git and agent sparklines each get their
    // own labeled zone row instead of sharing line 1, so a card's facts and the sparkline
    // that summarizes them are never split across unrelated lines.
    let mut p = project("p1", "spark-project", StatusBucket::Active);
    p.agent.active_agent = Some("claude-code".to_string());
    p.agent_activity = vec![0]; // flat -- isolates the git sparkline from the agent one below
    p.git.branch = Some("main".to_string());
    p.git.daily_commits = vec![0, 0, 5, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 2];
    let radar = radar_of(vec![p]);
    let state = DashboardState::new(&radar);
    let lines = rendered_lines(&radar, &state, 100, 40);

    // Find the card's header (name) line by content rather than a pinned index — the card is
    // now wrapped in a bordered `Block`, which shifts every line below it by one row, and a
    // future chrome tweak shouldn't need this test rewritten a third time.
    let header_idx = lines
        .iter()
        .position(|l| l.contains("spark-project"))
        .expect("the card header must show the project name somewhere on screen");
    let header_line = &lines[header_idx];
    let git_row = &lines[header_idx + 1];
    let agent_row = &lines[header_idx + 2];

    assert!(
        header_line.contains("spark-project"),
        "the card header must show the project name, got:\n{header_line:?}"
    );
    assert!(
        git_row.contains("git") && git_row.contains("main"),
        "the git zone row must show its own label and branch, got:\n{git_row:?}"
    );
    assert!(
        git_row
            .chars()
            .any(|c| ('\u{2582}'..='\u{2588}').contains(&c)),
        "the git zone row must contain at least one non-zero-level git sparkline bar, got:\n{git_row:?}"
    );
    assert!(
        agent_row.contains("agent") && agent_row.contains("claude-code"),
        "the agent zone row must show its own label and the active agent, got:\n{agent_row:?}"
    );
}

// --- MECH-5: the "waiting on you" indicator --------------------------------

#[test]
fn roomy_card_for_a_waiting_project_says_waiting_on_you_instead_of_a_silence_age() {
    // The header's right-hand field normally carries `silent 12m`. For a waiting project
    // that number is the least informative thing on the row — the run is silent *because*
    // it is blocked — so the field says so outright.
    let mut p = project("p1", "blocked-project", StatusBucket::Active);
    p.agent.active_agent = Some("claude-code".to_string());
    p.last_activity_at = Some(chrono::Utc::now() - chrono::Duration::minutes(12));
    p.agent.waiting_since = Some(chrono::Utc::now() - chrono::Duration::minutes(11));
    let radar = radar_of(vec![p]);
    let state = DashboardState::new(&radar);
    let whole = rendered_lines(&radar, &state, 100, 40).join("\n");

    assert!(
        whole.to_lowercase().contains("waiting on you"),
        "a waiting project's roomy card must say so in words, got:\n{whole}"
    );
    assert!(
        !whole.contains("silent 12m"),
        "the silence age must be displaced, not shown alongside — it is the inference the \
         latch contradicts, got:\n{whole}"
    );
    assert!(
        whole.contains('▲'),
        "the ▲ marker (deliberately not ⚠, whose emoji-presentation default breaks column \
         alignment in most terminals) must render, got:\n{whole}"
    );
}

#[test]
fn compact_row_for_a_waiting_project_shows_the_marker_and_the_words() {
    // The compact tier is where the field budget is tightest, and it is also the tier a
    // short terminal drops into — i.e. exactly where a truncated `waiting on…` would be
    // easiest to ship unnoticed.
    let mut p = project("p1", "blocked-project", StatusBucket::Active);
    p.agent.active_agent = Some("claude-code".to_string());
    p.last_activity_at = Some(chrono::Utc::now() - chrono::Duration::minutes(12));
    p.agent.waiting_since = Some(chrono::Utc::now() - chrono::Duration::minutes(11));
    let radar = radar_of(vec![p]);
    let state = DashboardState::new(&radar);
    let whole = rendered_lines(&radar, &state, 100, 12).join("\n");

    assert!(
        whole.contains("waiting on you"),
        "the compact row must carry the full phrase, not an elided one, got:\n{whole}"
    );
    assert!(
        whole.contains('▲'),
        "compact rows carry the marker too, got:\n{whole}"
    );
}

#[test]
fn a_project_whose_latch_has_expired_renders_as_an_ordinary_silent_row() {
    // Same two-clocks rule the sort test pins: petri may be drawing a snapshot written
    // before the scanner's release tick, so the expiry has to be re-derived here.
    let mut p = project("p1", "stale-latch", StatusBucket::Active);
    p.agent.active_agent = Some("claude-code".to_string());
    p.last_activity_at = Some(chrono::Utc::now() - chrono::Duration::minutes(12));
    p.agent.waiting_since = Some(
        chrono::Utc::now()
            - chrono::Duration::seconds(petridish_core::schema::WAITING_MAX_LATCH_S + 60),
    );
    let radar = radar_of(vec![p]);
    let state = DashboardState::new(&radar);
    let whole = rendered_lines(&radar, &state, 100, 40).join("\n");

    assert!(
        !whole.to_lowercase().contains("waiting"),
        "an expired latch must not draw the indicator, got:\n{whole}"
    );
    assert!(
        whole.contains("silent 12m"),
        "and the ordinary silence age comes back, got:\n{whole}"
    );
}
