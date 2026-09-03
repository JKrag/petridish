//! SPACE-1 acceptance gate, phase B (`petri/IDEAS.md` §3) — protected, authored by the
//! orchestrator, not the delegate. Where this file and the prompt disagree, this file wins.
//!
//! Scope: the three pure functions phase B adds — `dashboard::feed_rows_for` (how much
//! surplus the feed may claim), `feed::feed_block_lines` (what it draws), and
//! `lib::absorb_snapshot` (the reload-order rule) — plus the rendered result on a real
//! `TestBackend`. The plumbing that calls them is already written and compiles.
//!
//! Structural, not exact-buffer, for the same reason `s4_snapshot.rs` gives: pinning every
//! cell makes the tests a transcription of the implementation instead of a statement about
//! it.

use petri::dashboard::{DashboardState, feed_rows_for, plan_layout};
use petri::feed::{FeedKind, FeedState, feed_block_lines};
use petridish_core::schema::{
    AgentActivity, AgentState, GitState, Project, Radar, StatusBucket,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};

fn ts(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .unwrap_or_else(|e| panic!("bad fixture timestamp {s}: {e}"))
        .with_timezone(&chrono::Utc)
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

fn with_agent(mut p: Project, who: &str, event: &str, at: &str) -> Project {
    p.agent = AgentState {
        state: AgentActivity::Working,
        active_agent: Some(who.to_string()),
        last_event: Some(event.to_string()),
        last_event_at: Some(ts(at)),
        session_id: Some("s1".to_string()),
    };
    p
}

fn radar_at(updated_at: &str, projects: Vec<Project>) -> Radar {
    Radar {
        schema_version: 1,
        updated_at: ts(updated_at),
        scan_duration_ms: 0,
        projects,
        quota: None,
    }
}

/// `n` IN FLIGHT projects — compact single-row members, so a section's height is easy to
/// reason about when setting up a truncation case.
fn fleet(n: usize) -> Vec<Project> {
    (0..n)
        .map(|i| project(&format!("p{i}"), &format!("proj-{i:02}"), StatusBucket::InFlight))
        .collect()
}

/// Every cell of a rendered frame, one `String` per row.
fn render_rows(radar: &Radar, feed: &FeedState, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test backend");
    let state = DashboardState::new(radar);
    terminal
        .draw(|frame| petri::dashboard::render(frame, radar, &state, feed))
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

fn joined(rows: &[String]) -> String {
    rows.join("\n")
}

/// A feed with `n` agent rows, newest last in insertion order.
///
/// Stamps hour `8 + i`, so `n` must stay under 16 or the hour runs past 23 and `ts` panics
/// on an out-of-range timestamp. Caught by `block_is_exactly_the_rows_it_was_given`, which
/// originally asked for 20.
fn feed_of(n: usize) -> FeedState {
    assert!(n < 16, "feed_of stamps hour 8+i; n={n} would run past 23:00");
    let projects: Vec<Project> = (0..n)
        .map(|i| {
            with_agent(
                project(&format!("p{i}"), &format!("proj-{i:02}"), StatusBucket::Active),
                "claude-code",
                "Stop",
                &format!("2026-09-03T{:02}:00:00Z", 8 + i),
            )
        })
        .collect();
    FeedState::seeded(&radar_at("2026-09-03T20:00:00Z", projects))
}

// ------------------------------------------------------------------ feed_rows_for

#[test]
fn feed_gets_nothing_in_the_compact_tier() {
    // Below COMPACT_TIER_MAX_CONTENT_ROWS the screen is already rationing space, which is
    // the opposite of the surplus SPACE-1 exists to spend.
    assert_eq!(feed_rows_for(true, 40, 4, &[], &[]), 0);
}

#[test]
fn feed_gets_nothing_when_a_section_was_skipped() {
    assert_eq!(
        feed_rows_for(false, 40, 4, &[], &[(StatusBucket::Cold, 7)]),
        0,
        "projects hidden entirely must outrank the feed"
    );
}

#[test]
fn feed_gets_the_surplus_when_the_whole_fleet_is_shown() {
    let rows = feed_rows_for(false, 40, 20, &[], &[]);
    assert!(rows > 0, "20 spare rows and nothing hidden: the feed should draw");
    assert!(rows <= 12, "feed must not claim more than FEED_MAX_ROWS, got {rows}");
}

#[test]
fn feed_gets_nothing_when_too_few_rows_remain() {
    // 3 spare rows cannot carry a rule + label + two events; an almost-empty labelled box
    // is worse than no box.
    assert_eq!(feed_rows_for(false, 40, 37, &[], &[]), 0);
    assert_eq!(feed_rows_for(false, 40, 40, &[], &[]), 0);
    assert_eq!(feed_rows_for(false, 40, 41, &[], &[]), 0, "must not underflow past `used`");
}

#[test]
fn feed_yields_to_a_section_that_truncated_its_own_rows() {
    // The case that motivates the rule and that no naive check catches: `plan_layout` can
    // leave surplus WHILE truncating, because a roomy card spans 7 rows. A tall-but-narrow
    // terminal with many RUNNING projects is exactly that shape.
    let projects: Vec<Project> = (0..24)
        .map(|i| {
            with_agent(
                project(&format!("r{i}"), &format!("run-{i:02}"), StatusBucket::Active),
                "claude-code",
                "PreToolUse",
                "2026-09-03T09:00:00Z",
            )
        })
        .collect();
    let radar = radar_at("2026-09-03T09:30:00Z", projects);
    let plan = plan_layout(Rect::new(0, 0, 70, 30), &radar, [false, false, false, false]);

    let truncated = plan.sections.iter().any(|s| s.truncated_remaining.is_some());
    let skipped = !plan.skipped.is_empty();
    assert!(
        truncated || skipped,
        "fixture must actually overflow for this test to mean anything"
    );
    assert_eq!(
        plan.feed_rows, 0,
        "the feed must not take rows while a `… +N more` marker is on screen"
    );
}

// ------------------------------------------------------------------ feed_block_lines

#[test]
fn block_is_exactly_the_rows_it_was_given() {
    for rows in [4usize, 6, 12] {
        let lines = feed_block_lines(&feed_of(15), 80, rows);
        assert_eq!(lines.len(), rows, "block must fill its rect exactly at rows={rows}");
    }
}

#[test]
fn block_is_empty_below_three_rows() {
    assert!(feed_block_lines(&feed_of(5), 80, 2).is_empty());
    assert!(feed_block_lines(&feed_of(5), 80, 0).is_empty());
}

#[test]
fn block_is_labelled_and_lists_newest_first() {
    let lines = feed_block_lines(&feed_of(6), 80, 6);
    let text: Vec<String> = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect();

    assert!(
        text.iter().any(|l| l.contains("ACTIVITY")),
        "the block must say what it is, got {text:?}"
    );
    // feed_of(6) stamps proj-00 at 08:00 .. proj-05 at 13:00, so the newest is proj-05.
    let body: Vec<&String> = text.iter().filter(|l| l.contains("proj-")).collect();
    assert!(body.len() >= 2, "expected event rows, got {text:?}");
    assert!(
        body[0].contains("proj-05"),
        "newest event must be the first body row, got {:?}",
        body[0]
    );
    assert!(
        body[0].contains("13:00"),
        "rows carry their clock, got {:?}",
        body[0]
    );
}

#[test]
fn block_says_so_when_there_is_nothing_to_show() {
    // An unexplained empty box reads as breakage; "nothing has happened" is the honest
    // reading of a quiet fleet and must be stated.
    let lines = feed_block_lines(&FeedState::default(), 80, 5);
    let text: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect::<Vec<_>>()
        .join("");
    assert_eq!(lines.len(), 5);
    assert!(
        text.to_lowercase().contains("no")
            || text.to_lowercase().contains("nothing")
            || text.to_lowercase().contains("quiet"),
        "empty feed must explain itself, got {text:?}"
    );
}

#[test]
fn rows_are_truncated_to_width_never_wrapped() {
    // The header/footer of this screen cannot wrap either (ACT-10's slice found the same
    // constraint); a row wider than the pane would push content off the edge silently.
    let mut feed = FeedState::default();
    feed.ingest(
        &radar_at("2026-09-03T08:00:00Z", vec![project("a", "alpha", StatusBucket::Cold)]),
        &radar_at(
            "2026-09-03T09:00:00Z",
            vec![
                project("a", "alpha", StatusBucket::Cold),
                project("b", &"very-long-project-name-".repeat(6), StatusBucket::Cold),
            ],
        ),
    );
    let lines = feed_block_lines(&feed, 40, 5);
    for line in &lines {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            text.chars().count() <= 40,
            "line exceeds the 40-column budget: {text:?}"
        );
    }
}

#[test]
fn block_does_not_panic_on_a_hostile_width() {
    for width in [0usize, 1, 3, 10] {
        let _ = feed_block_lines(&feed_of(4), width, 5);
    }
}

// ------------------------------------------------------------------ absorb_snapshot

#[test]
fn absorb_seeds_the_feed_on_the_very_first_snapshot() {
    let fresh = radar_at(
        "2026-09-03T09:00:00Z",
        vec![with_agent(project("a", "alpha", StatusBucket::Active), "claude-code", "Stop", "2026-09-03T08:55:00Z")],
    );
    let mut feed = FeedState::default();
    let last_good = petri::absorb_snapshot(&mut feed, None, fresh);

    assert!(last_good.is_some(), "the fresh snapshot becomes last_good");
    assert_eq!(feed.len(), 1, "a first read seeds rather than diffing against nothing");
    assert_eq!(feed.events().front().unwrap().kind, FeedKind::Agent);
}

#[test]
fn absorb_diffs_against_the_previous_snapshot_before_replacing_it() {
    // The trap this whole function exists for: assigning last_good first would leave the
    // feed permanently empty, on a path no unit test naturally reaches.
    let prev = radar_at(
        "2026-09-03T09:00:00Z",
        vec![with_agent(project("a", "alpha", StatusBucket::Active), "claude-code", "PreToolUse", "2026-09-03T08:55:00Z")],
    );
    let next = radar_at(
        "2026-09-03T09:05:00Z",
        vec![with_agent(project("a", "alpha", StatusBucket::Active), "claude-code", "Stop", "2026-09-03T09:04:00Z")],
    );

    let mut feed = FeedState::default();
    let last_good = petri::absorb_snapshot(&mut feed, Some(prev), next);

    assert_eq!(feed.len(), 1, "the advance between the two snapshots must produce a row");
    let e = feed.events().front().unwrap();
    assert_eq!(e.at, ts("2026-09-03T09:04:00Z"));
    assert_eq!(e.detail, "claude-code stop");
    assert_eq!(
        last_good.expect("last_good returned").updated_at,
        ts("2026-09-03T09:05:00Z"),
        "the returned snapshot is the fresh one, not the old one"
    );
}

#[test]
fn absorb_accumulates_across_several_reloads() {
    let mut feed = FeedState::default();
    let mut last_good: Option<Radar> = None;
    for i in 0..4 {
        let fresh = radar_at(
            &format!("2026-09-03T{:02}:30:00Z", 9 + i),
            vec![with_agent(
                project("a", "alpha", StatusBucket::Active),
                "claude-code",
                "Stop",
                &format!("2026-09-03T{:02}:00:00Z", 9 + i),
            )],
        );
        last_good = petri::absorb_snapshot(&mut feed, last_good, fresh);
    }
    // One seeded row, then three diffs.
    assert_eq!(feed.len(), 4);
    assert_eq!(feed.events().front().unwrap().at, ts("2026-09-03T12:00:00Z"));
}

// ------------------------------------------------------------------ rendered result

#[test]
fn a_tall_dashboard_draws_the_feed() {
    let radar = radar_at("2026-09-03T20:00:00Z", fleet(4));
    let screen = joined(&render_rows(&radar, &feed_of(6), 100, 44));
    assert!(screen.contains("ACTIVITY"), "tall terminal, small fleet: expected a feed block");
    assert!(screen.contains("proj-05"), "expected the newest feed row on screen");
}

#[test]
fn a_short_dashboard_draws_no_feed() {
    let radar = radar_at("2026-09-03T20:00:00Z", fleet(4));
    let screen = joined(&render_rows(&radar, &feed_of(6), 100, 14));
    assert!(
        !screen.contains("ACTIVITY"),
        "compact tier must spend every row on the fleet"
    );
}

#[test]
fn the_feed_sits_below_the_fleet_not_above_it() {
    let radar = radar_at("2026-09-03T20:00:00Z", fleet(4));
    let rows = render_rows(&radar, &feed_of(6), 100, 44);
    let activity = rows.iter().position(|r| r.contains("ACTIVITY")).expect("feed drawn");
    // Compare against the section header rather than a project row: feed rows also carry
    // project names, so "the last row mentioning a project" is not a fleet landmark.
    let section = rows
        .iter()
        .position(|r| r.contains("IN FLIGHT"))
        .expect("the IN FLIGHT section header must be on screen");
    assert!(
        activity > section,
        "feed at row {activity} must come below the fleet, whose section header is at {section}"
    );
}

#[test]
fn rendering_with_a_feed_does_not_panic_at_hostile_sizes() {
    let radar = radar_at("2026-09-03T20:00:00Z", fleet(30));
    for (w, h) in [(40u16, 10u16), (1, 1), (80, 24), (200, 60), (40, 44)] {
        let _ = render_rows(&radar, &feed_of(8), w, h);
    }
}

// ------------------------------------------- orchestrator follow-ups (post-delegation)
//
// Two holes in the delegated `feed_block_lines`, found by reading it. The second is what
// the gate's clippy clause caught as a dead `kind_color`; the first the tests missed
// because every fixture above happens to supply more events than the block has room for.

#[test]
fn block_pads_a_short_feed_to_its_full_height() {
    // The empty-feed branch pads with blank lines; the populated branch must too, or a
    // fleet with three events in a twelve-row block returns five lines and the contract
    // ("exactly `rows` lines") quietly stops holding. Same asymmetry as phase A's commit
    // arm: one branch handling a case its sibling drops.
    let lines = feed_block_lines(&feed_of(2), 80, 8);
    assert_eq!(lines.len(), 8, "a short feed must still fill its rect");
}

#[test]
fn rows_are_tinted_by_what_produced_them() {
    // Structural events are rarer than agent chatter and are meant to stand out of it
    // without a second column. Untinted rows render the distinction invisible.
    let mut feed = FeedState::default();
    feed.ingest(
        &radar_at(
            "2026-09-03T08:00:00Z",
            vec![with_agent(project("a", "alpha", StatusBucket::Active), "claude-code", "Stop", "2026-09-03T07:00:00Z")],
        ),
        &radar_at(
            "2026-09-03T09:00:00Z",
            vec![with_agent(project("a", "alpha", StatusBucket::InFlight), "claude-code", "Stop", "2026-09-03T08:30:00Z")],
        ),
    );
    // One Bucket row and one Agent row, in that order (newest first).
    let kinds: Vec<FeedKind> = feed.events().iter().map(|e| e.kind).collect();
    assert_eq!(kinds, vec![FeedKind::Bucket, FeedKind::Agent], "fixture precondition");

    let lines = feed_block_lines(&feed, 80, 4);
    let body_styles: Vec<_> = lines[2..4].iter().map(|l| l.spans[0].style.fg).collect();
    assert_ne!(
        body_styles[0], body_styles[1],
        "a bucket-change row and an agent row must not render identically"
    );
}
