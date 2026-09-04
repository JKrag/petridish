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

/// The foreground colour of span `i` of `line`, failing with a readable message rather than
/// an index panic when the line is a padded blank (`Line::default()`, zero spans).
fn span_fg(line: &ratatui::text::Line<'_>, i: usize) -> Option<ratatui::style::Color> {
    let span = line.spans.get(i).unwrap_or_else(|| {
        panic!(
            "expected a span {i} on this row, but it has {} — a padded blank row means the \
             fixture is shorter than the block",
            line.spans.len()
        )
    });
    span.style.fg
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
fn feed_takes_leftover_rows_even_when_something_was_hidden() {
    // Relaxed deliberately. Rows left after planning cannot be used by any section — a
    // truncated one stopped because the remainder is smaller than one more item, and a
    // skipped one because fewer than its 3 chrome rows remained. Refusing them reserved
    // blank rows rather than protecting project rows.
    assert_eq!(
        feed_rows_for(false, 40, 4, &[], &[(StatusBucket::Cold, 7)], 30),
        31,
        "leftover after a skip is still leftover (30 events + the label)"
    );
    assert_eq!(
        feed_rows_for(true, 40, 4, &[], &[], 30),
        31,
        "and the compact tier has no more use for a blank row than a tall one does"
    );
}

#[test]
fn feed_gets_the_whole_surplus_when_there_is_activity_to_fill_it() {
    // SPACE-1 is "fill the slack". An earlier version capped this at 12 rows, which left
    // two thirds of a 30-row surplus blank — the exact complaint the idea exists to answer.
    assert_eq!(
        feed_rows_for(false, 40, 20, &[], &[], 50),
        20,
        "with plenty of activity the feed takes every spare row"
    );
    assert_eq!(
        feed_rows_for(false, 60, 20, &[], &[], 50),
        40,
        "and keeps taking them on a much taller terminal — there is no fixed ceiling"
    );
}

#[test]
fn feed_claims_only_what_it_can_fill() {
    // The other half of the rule: surplus it would fill with blank rows is better given
    // back to the layout than fenced off inside an mostly-empty block.
    assert_eq!(
        feed_rows_for(false, 40, 10, &[], &[], 5),
        6,
        "5 events plus the label"
    );
    assert_eq!(
        feed_rows_for(false, 40, 10, &[], &[], 0),
        3,
        "an empty feed still gets enough rows to say it is empty"
    );
}

#[test]
fn feed_gets_nothing_when_too_few_rows_remain() {
    // 3 spare rows cannot carry a rule + label + two events; an almost-empty labelled box
    // is worse than no box.
    assert_eq!(feed_rows_for(false, 40, 38, &[], &[], 30), 0, "2 spare rows is below the floor");
    assert_eq!(feed_rows_for(false, 40, 37, &[], &[], 30), 3, "3 is exactly the floor");
    assert_eq!(feed_rows_for(false, 40, 40, &[], &[], 30), 0);
    assert_eq!(feed_rows_for(false, 40, 41, &[], &[], 30), 0, "must not underflow past `used`");
}

#[test]
fn leftover_beside_a_truncated_section_is_too_small_for_another_card() {
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
    let plan = plan_layout(Rect::new(0, 0, 70, 30), &radar, [false, false, false, false], 30);

    let truncated = plan.sections.iter().any(|s| s.truncated_remaining.is_some());
    let skipped = !plan.skipped.is_empty();
    assert!(
        truncated || skipped,
        "fixture must actually overflow for this test to mean anything"
    );
    // Whatever those leftover rows are, they are fewer than one roomy card (7) — which is
    // precisely why the section stopped — so no project row could have used them.
    assert!(
        plan.feed_rows < 7,
        "leftover beside a truncated section must be smaller than one card, got {}",
        plan.feed_rows
    );
}

// ------------------------------------------------------------------ feed_block_lines

#[test]
fn block_is_exactly_the_rows_it_was_given() {
    for rows in [4usize, 6, 12] {
        let lines = feed_block_lines(&feed_of(15), ts("2026-09-03T20:00:00Z"), 80, rows);
        assert_eq!(lines.len(), rows, "block must fill its rect exactly at rows={rows}");
    }
}

#[test]
fn block_is_empty_below_two_rows() {
    // A label with no room for one event is not worth the row.
    assert!(feed_block_lines(&feed_of(5), ts("2026-09-03T20:00:00Z"), 80, 1).is_empty());
    assert!(feed_block_lines(&feed_of(5), ts("2026-09-03T20:00:00Z"), 80, 0).is_empty());
}

#[test]
fn block_draws_no_rule_of_its_own() {
    // Whatever sits above always ends in one; a second produced two identical dividers in
    // consecutive rows on a real dashboard.
    let lines = feed_block_lines(&feed_of(4), ts("2026-09-03T20:00:00Z"), 40, 5);
    let first: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        first.contains("ACTIVITY"),
        "the block must start at its label, got {first:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>().contains("──")),
        "the block must draw no rule"
    );
}

#[test]
fn block_is_labelled_and_lists_newest_first() {
    let lines = feed_block_lines(&feed_of(6), ts("2026-09-03T20:00:00Z"), 80, 6);
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
    let lines = feed_block_lines(&FeedState::default(), ts("2026-09-03T20:00:00Z"), 80, 5);
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
    let lines = feed_block_lines(&feed, ts("2026-09-03T20:00:00Z"), 40, 5);
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
        let _ = feed_block_lines(&feed_of(4), ts("2026-09-03T20:00:00Z"), width, 5);
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
fn a_short_dashboard_still_spends_its_leftover_on_the_feed() {
    // Retired premise: this used to assert the compact tier never draws a feed. Four
    // projects fit in a 14-row terminal with rows to spare, and those rows were blank —
    // a small terminal has no more use for a blank row than a tall one does.
    let radar = radar_at("2026-09-03T20:00:00Z", fleet(4));
    let screen = joined(&render_rows(&radar, &feed_of(6), 100, 14));
    assert!(screen.contains("ACTIVITY"), "leftover rows should carry the feed");
    for i in 0..4 {
        assert!(
            screen.contains(&format!("proj-{i:02}")),
            "every project must still be on screen: proj-{i:02}"
        );
    }
}

#[test]
fn a_full_dashboard_draws_no_feed() {
    // The case that actually matters: when the fleet consumes the budget there is nothing
    // left, and the feed must not take a row from it.
    let radar = radar_at("2026-09-03T20:00:00Z", fleet(40));
    let rows = render_rows(&radar, &feed_of(6), 100, 14);
    assert!(
        !joined(&rows).contains("ACTIVITY"),
        "a screen with no spare rows gets no feed"
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
    let lines = feed_block_lines(&feed_of(2), ts("2026-09-03T20:00:00Z"), 80, 8);
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

    let lines = feed_block_lines(&feed, ts("2026-09-03T20:00:00Z"), 80, 4);
    // span 0 is the time field, span 1 the body — kind tints the body. Indexed through a
    // helper because a PADDED body row is `Line::default()` with zero spans: a fixture
    // change that shortens the feed would otherwise turn this into an index panic instead
    // of a legible failure. Flagged by the delegate's own review of these tests.
    let body_styles: Vec<_> = lines[1..3].iter().map(|l| span_fg(l, 1)).collect();
    assert_ne!(
        body_styles[0], body_styles[1],
        "a bucket-change row and an agent row must not render identically"
    );
}

#[test]
fn a_clock_and_a_date_are_visually_distinct() {
    // Two rows, one from today and one from an earlier day, otherwise identical in kind.
    // `MM-DD` versus `HH:MM` alone is too weak a cue to catch while glancing, which is the
    // only way this block is ever read.
    let mut feed = FeedState::default();
    feed.ingest(
        &radar_at("2026-09-01T08:00:00Z", vec![project("a", "alpha", StatusBucket::Active)]),
        &radar_at(
            "2026-09-02T23:14:00Z",
            vec![with_agent(project("a", "alpha", StatusBucket::Active), "claude-code", "Stop", "2026-09-02T23:14:00Z")],
        ),
    );
    feed.ingest(
        &radar_at(
            "2026-09-02T23:15:00Z",
            vec![with_agent(project("b", "bravo", StatusBucket::Active), "claude-code", "Stop", "2026-09-02T23:14:00Z")],
        ),
        &radar_at(
            "2026-09-03T04:58:00Z",
            vec![with_agent(project("b", "bravo", StatusBucket::Active), "claude-code", "Stop", "2026-09-03T04:58:00Z")],
        ),
    );

    let now = ts("2026-09-03T09:00:00Z");
    let lines = feed_block_lines(&feed, now, 80, 4);
    let stamps: Vec<_> = lines[1..3].iter().map(|l| span_fg(l, 0)).collect();
    let texts: Vec<String> = lines[1..3]
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();

    assert!(texts[0].contains("04:58"), "newest row is today's clock, got {:?}", texts[0]);
    assert!(texts[1].contains("09-02"), "older row is a date, got {:?}", texts[1]);
    assert_ne!(
        stamps[0], stamps[1],
        "today's clock and an earlier day's date must not share a colour"
    );
}

#[test]
fn the_stamp_survives_a_narrow_pane_before_the_body_does() {
    // The stamp is what makes the block chronological, so it is the last thing a narrow
    // pane should lose. Also guards the two-span truncation against exceeding the budget.
    for width in [0usize, 1, 4, 8, 12, 40] {
        let lines = feed_block_lines(&feed_of(3), ts("2026-09-03T20:00:00Z"), width, 5);
        for line in &lines {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(
                text.chars().count() <= width,
                "line exceeds the {width}-column budget: {text:?}"
            );
        }
    }
}
