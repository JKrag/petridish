//! SPACE-1 acceptance gate, phase A (`petri/IDEAS.md` §3) — protected, authored by the
//! orchestrator, not the delegate. Same convention as `s6_snapshot.rs`: where this file and
//! the prompt disagree, this file wins.
//!
//! Scope: the pure half of the feed — `humanize_event`, `agent_detail`, `FeedEvent::row_text`,
//! `FeedState::seeded` and `FeedState::ingest`. Nothing here touches ratatui; the layout and
//! drawing half is phase B (`s9_feed_render.rs`).
//!
//! Every test below FAILS (panics on `todo!()`) against the stub — confirmed before
//! delegating.

use petri::feed::{FeedKind, FeedState, agent_detail, humanize_event};
use petridish_core::schema::{
    AgentActivity, AgentState, GitState, Project, Radar, StatusBucket,
};

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

/// A project with an agent event at `at`, by `who`, named `event`.
fn with_agent(mut p: Project, who: &str, event: &str, at: &str) -> Project {
    p.agent = AgentState {
        state: AgentActivity::Working,
        active_agent: Some(who.to_string()),
        last_event: Some(event.to_string()),
        last_event_at: Some(ts(at)),
        session_id: Some("s1".to_string()),
        waiting_since: None,
    };
    p
}

/// A dirty repo with `files` uncommitted files on `branch`, last commit at `commit_at`.
fn with_git(mut p: Project, branch: &str, files: u32, commit_at: Option<&str>) -> Project {
    p.git = GitState {
        is_repo: true,
        branch: Some(branch.to_string()),
        is_dirty: files > 0,
        uncommitted_files: files,
        last_commit_at: commit_at.map(ts),
        mine_last_commit_at: None,
        github_url: None,
        daily_commits: Vec::new(),
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

// ---------------------------------------------------------------- humanize_event

#[test]
fn humanize_event_splits_camel_case_and_lowercases() {
    assert_eq!(humanize_event("PreToolUse"), "pre tool use");
    assert_eq!(humanize_event("Stop"), "stop");
    assert_eq!(humanize_event("SessionStart"), "session start");
}

#[test]
fn humanize_event_splits_underscores_too() {
    assert_eq!(humanize_event("user_prompt_submit"), "user prompt submit");
    assert_eq!(humanize_event("Notification_Sent"), "notification sent");
}

#[test]
fn humanize_event_keeps_capital_runs_together() {
    // A run of consecutive capitals is one word, so an acronym doesn't explode into
    // single letters.
    assert_eq!(humanize_event("HTTPStart"), "http start");
}

#[test]
fn humanize_event_falls_back_when_nothing_is_left() {
    // Never return an empty string — a row would render a dangling separator.
    assert_eq!(humanize_event(""), "activity");
    assert_eq!(humanize_event("___"), "activity");
}

// ---------------------------------------------------------------- agent_detail

#[test]
fn agent_detail_names_the_agent_and_the_event() {
    let p = with_agent(project("a", "radar", StatusBucket::Active), "claude-code", "Stop", "2026-09-03T14:22:00Z");
    assert_eq!(agent_detail(&p), "claude-code stop");
}

#[test]
fn agent_detail_appends_the_dirty_file_count_with_correct_grammar() {
    let base = with_agent(project("a", "radar", StatusBucket::Active), "copilot", "PreToolUse", "2026-09-03T14:22:00Z");

    let three = with_git(base.clone(), "main", 3, None);
    assert_eq!(agent_detail(&three), "copilot pre tool use · 3 files");

    let one = with_git(base.clone(), "main", 1, None);
    assert_eq!(agent_detail(&one), "copilot pre tool use · 1 file");

    // Clean repo: no suffix at all.
    let clean = with_git(base.clone(), "main", 0, None);
    assert_eq!(agent_detail(&clean), "copilot pre tool use");

    // Not a repo: no suffix either.
    assert_eq!(agent_detail(&base), "copilot pre tool use");
}

#[test]
fn agent_detail_is_agent_agnostic_when_the_agent_is_unknown() {
    // IDEAS.md §5: never name a specific vendor as a fallback.
    let mut p = project("a", "radar", StatusBucket::Active);
    p.agent.last_event = Some("Stop".to_string());
    p.agent.last_event_at = Some(ts("2026-09-03T14:22:00Z"));
    let detail = agent_detail(&p);
    assert_eq!(detail, "agent stop");
    assert!(
        !detail.to_lowercase().contains("claude"),
        "fallback must not name a vendor, got {detail:?}"
    );
}

#[test]
fn agent_detail_falls_back_when_the_event_name_is_missing() {
    let mut p = project("a", "radar", StatusBucket::Active);
    p.agent.active_agent = Some("claude-code".to_string());
    assert_eq!(agent_detail(&p), "claude-code activity");
}

// ---------------------------------------------------------------- row_text

#[test]
fn row_text_is_clock_project_then_detail() {
    let p = with_git(
        with_agent(project("a", "project-radar", StatusBucket::Active), "claude-code", "Stop", "2026-09-03T14:22:07Z"),
        "main",
        3,
        None,
    );
    let feed = FeedState::seeded(&radar_at("2026-09-03T14:30:00Z", vec![p]));
    let row = feed
        .events()
        .front()
        .expect("one seeded row")
        .row_text(ts("2026-09-03T14:30:00Z"));
    // The shape IDEAS.md's SPACE-1 entry names, on a UTC minute-precision clock.
    assert_eq!(row, "14:22  project-radar · claude-code stop · 3 files");
}

#[test]
fn row_text_shows_a_date_for_an_event_from_an_earlier_day() {
    // Found against real data: a fleet quiet overnight renders yesterday's 23:14 below
    // today's 04:58, which reads as a sorting bug even though the order is right. A bare
    // clock cannot say "yesterday". Same five columns, so rows stay aligned.
    let p = with_agent(
        project("a", "project-radar", StatusBucket::Active),
        "claude-code",
        "Stop",
        "2026-09-02T23:14:00Z",
    );
    let feed = FeedState::seeded(&radar_at("2026-09-03T05:00:00Z", vec![p]));
    let row = feed.events().front().unwrap().row_text(ts("2026-09-03T05:00:00Z"));
    assert_eq!(row, "09-02  project-radar · claude-code stop");
}

// ---------------------------------------------------------------- seeded

#[test]
fn seeded_emits_one_row_per_project_with_an_agent_timestamp_newest_first() {
    let old = with_agent(project("a", "alpha", StatusBucket::Active), "claude-code", "Stop", "2026-09-03T09:00:00Z");
    let new = with_agent(project("b", "bravo", StatusBucket::Active), "claude-code", "PreToolUse", "2026-09-03T11:00:00Z");
    let silent = project("c", "charlie", StatusBucket::Cold);

    let feed = FeedState::seeded(&radar_at("2026-09-03T12:00:00Z", vec![old, new, silent]));

    assert_eq!(feed.len(), 2, "the project with no agent timestamp contributes no row");
    let names: Vec<&str> = feed.events().iter().map(|e| e.project.as_str()).collect();
    assert_eq!(names, vec!["bravo", "alpha"], "newest first");
    assert!(feed.events().iter().all(|e| e.kind == FeedKind::Agent));
    assert!(
        !feed.events().iter().any(|e| e.project == "charlie"),
        "a project with no agent event must not be invented into the feed"
    );
}

#[test]
fn seeded_breaks_timestamp_ties_by_project_name() {
    let b = with_agent(project("b", "bravo", StatusBucket::Active), "claude-code", "Stop", "2026-09-03T09:00:00Z");
    let a = with_agent(project("a", "alpha", StatusBucket::Active), "claude-code", "Stop", "2026-09-03T09:00:00Z");
    // Deliberately out of name order in the input.
    let feed = FeedState::seeded(&radar_at("2026-09-03T12:00:00Z", vec![b, a]));
    let names: Vec<&str> = feed.events().iter().map(|e| e.project.as_str()).collect();
    assert_eq!(names, vec!["alpha", "bravo"], "ties resolve by name ascending, not input order");
}

#[test]
fn seeded_is_empty_for_a_fleet_with_no_agent_activity() {
    let feed = FeedState::seeded(&radar_at("2026-09-03T12:00:00Z", vec![project("a", "alpha", StatusBucket::Cold)]));
    assert!(feed.is_empty());
    assert_eq!(feed.len(), 0);
}

// ---------------------------------------------------------------- ingest

#[test]
fn ingest_emits_nothing_when_nothing_changed() {
    let p = with_agent(project("a", "alpha", StatusBucket::Active), "claude-code", "Stop", "2026-09-03T09:00:00Z");
    let prev = radar_at("2026-09-03T12:00:00Z", vec![p.clone()]);
    let next = radar_at("2026-09-03T12:01:00Z", vec![p]);

    let mut feed = FeedState::default();
    feed.ingest(&prev, &next);
    assert!(feed.is_empty(), "a tick where nothing moved must not produce rows");
}

#[test]
fn ingest_emits_an_agent_row_when_the_agent_timestamp_advances() {
    let before = with_agent(project("a", "alpha", StatusBucket::Active), "claude-code", "PreToolUse", "2026-09-03T09:00:00Z");
    let after = with_git(
        with_agent(project("a", "alpha", StatusBucket::Active), "claude-code", "Stop", "2026-09-03T09:05:00Z"),
        "main",
        2,
        None,
    );
    let prev = radar_at("2026-09-03T09:00:30Z", vec![before]);
    let next = radar_at("2026-09-03T09:05:30Z", vec![after]);

    let mut feed = FeedState::default();
    feed.ingest(&prev, &next);

    assert_eq!(feed.len(), 1);
    let e = feed.events().front().unwrap();
    assert_eq!(e.kind, FeedKind::Agent);
    assert_eq!(e.project, "alpha");
    // Stamped with the agent event's own time, NOT the snapshot's updated_at.
    assert_eq!(e.at, ts("2026-09-03T09:05:00Z"));
    assert_eq!(e.detail, "claude-code stop · 2 files");
}

#[test]
fn ingest_treats_a_first_ever_agent_timestamp_as_an_advance() {
    // None -> Some must count: `None` is older than any `Some`.
    let before = project("a", "alpha", StatusBucket::Active);
    let after = with_agent(project("a", "alpha", StatusBucket::Active), "claude-code", "Stop", "2026-09-03T09:05:00Z");
    let mut feed = FeedState::default();
    feed.ingest(
        &radar_at("2026-09-03T09:00:00Z", vec![before]),
        &radar_at("2026-09-03T09:06:00Z", vec![after]),
    );
    assert_eq!(feed.len(), 1);
    assert_eq!(feed.events().front().unwrap().kind, FeedKind::Agent);
}

#[test]
fn ingest_emits_a_commit_row_naming_the_branch() {
    let before = with_git(project("a", "alpha", StatusBucket::Active), "main", 0, Some("2026-09-03T08:00:00Z"));
    let after = with_git(project("a", "alpha", StatusBucket::Active), "main", 0, Some("2026-09-03T09:30:00Z"));
    let mut feed = FeedState::default();
    feed.ingest(
        &radar_at("2026-09-03T08:01:00Z", vec![before]),
        &radar_at("2026-09-03T09:31:00Z", vec![after]),
    );
    assert_eq!(feed.len(), 1);
    let e = feed.events().front().unwrap();
    assert_eq!(e.kind, FeedKind::Commit);
    assert_eq!(e.at, ts("2026-09-03T09:30:00Z"));
    assert_eq!(e.detail, "commit on main");
}

#[test]
fn ingest_commit_row_falls_back_when_there_is_no_branch() {
    let mut before = with_git(project("a", "alpha", StatusBucket::Active), "main", 0, Some("2026-09-03T08:00:00Z"));
    before.git.branch = None;
    let mut after = with_git(project("a", "alpha", StatusBucket::Active), "main", 0, Some("2026-09-03T09:30:00Z"));
    after.git.branch = None;
    let mut feed = FeedState::default();
    feed.ingest(
        &radar_at("2026-09-03T08:01:00Z", vec![before]),
        &radar_at("2026-09-03T09:31:00Z", vec![after]),
    );
    assert_eq!(feed.events().front().unwrap().detail, "commit on detached");
}

#[test]
fn ingest_emits_a_bucket_row_on_a_section_change() {
    let before = project("a", "alpha", StatusBucket::Active);
    let after = project("a", "alpha", StatusBucket::InFlight);
    let mut feed = FeedState::default();
    feed.ingest(
        &radar_at("2026-09-03T08:00:00Z", vec![before]),
        &radar_at("2026-09-03T09:00:00Z", vec![after]),
    );
    assert_eq!(feed.len(), 1);
    let e = feed.events().front().unwrap();
    assert_eq!(e.kind, FeedKind::Bucket);
    // No timestamp of its own — stamped with the snapshot that revealed it.
    assert_eq!(e.at, ts("2026-09-03T09:00:00Z"));
    assert_eq!(e.detail, "active → in_flight");
}

#[test]
fn ingest_emits_an_appeared_row_for_a_project_new_to_the_scan() {
    let prev = radar_at("2026-09-03T08:00:00Z", vec![project("a", "alpha", StatusBucket::Active)]);
    let next = radar_at(
        "2026-09-03T09:00:00Z",
        vec![project("a", "alpha", StatusBucket::Active), project("b", "bravo", StatusBucket::Cold)],
    );
    let mut feed = FeedState::default();
    feed.ingest(&prev, &next);
    assert_eq!(feed.len(), 1);
    let e = feed.events().front().unwrap();
    assert_eq!(e.kind, FeedKind::Appeared);
    assert_eq!(e.project, "bravo");
    assert_eq!(e.detail, "discovered");
    assert_eq!(e.at, ts("2026-09-03T09:00:00Z"));
}

#[test]
fn ingest_ignores_a_project_that_disappeared() {
    let prev = radar_at(
        "2026-09-03T08:00:00Z",
        vec![project("a", "alpha", StatusBucket::Active), project("b", "bravo", StatusBucket::Cold)],
    );
    let next = radar_at("2026-09-03T09:00:00Z", vec![project("a", "alpha", StatusBucket::Active)]);
    let mut feed = FeedState::default();
    feed.ingest(&prev, &next);
    assert!(feed.is_empty(), "a project leaving the scan is not something the user did");
}

#[test]
fn ingest_can_emit_several_rows_for_one_project_in_one_tick() {
    let before = with_git(
        with_agent(project("a", "alpha", StatusBucket::Active), "claude-code", "PreToolUse", "2026-09-03T09:00:00Z"),
        "main",
        1,
        Some("2026-09-03T08:00:00Z"),
    );
    let after = with_git(
        with_agent(project("a", "alpha", StatusBucket::InFlight), "claude-code", "Stop", "2026-09-03T09:10:00Z"),
        "main",
        0,
        Some("2026-09-03T09:20:00Z"),
    );
    let mut feed = FeedState::default();
    feed.ingest(
        &radar_at("2026-09-03T09:01:00Z", vec![before]),
        &radar_at("2026-09-03T09:30:00Z", vec![after]),
    );

    let kinds: Vec<FeedKind> = feed.events().iter().map(|e| e.kind).collect();
    assert_eq!(feed.len(), 3, "agent + commit + bucket, got {kinds:?}");
    // Newest first by the events' own timestamps: bucket (09:30, the snapshot time),
    // commit (09:20), agent (09:10).
    assert_eq!(kinds, vec![FeedKind::Bucket, FeedKind::Commit, FeedKind::Agent]);
}

#[test]
fn ingest_pushes_newer_rows_in_front_of_older_ones() {
    let a0 = with_agent(project("a", "alpha", StatusBucket::Active), "claude-code", "Stop", "2026-09-03T09:00:00Z");
    let a1 = with_agent(project("a", "alpha", StatusBucket::Active), "claude-code", "Stop", "2026-09-03T09:05:00Z");
    let a2 = with_agent(project("a", "alpha", StatusBucket::Active), "claude-code", "Stop", "2026-09-03T09:10:00Z");

    let r0 = radar_at("2026-09-03T09:00:30Z", vec![a0]);
    let r1 = radar_at("2026-09-03T09:05:30Z", vec![a1]);
    let r2 = radar_at("2026-09-03T09:10:30Z", vec![a2]);

    let mut feed = FeedState::default();
    feed.ingest(&r0, &r1);
    feed.ingest(&r1, &r2);

    let times: Vec<String> = feed.events().iter().map(|e| e.at.to_rfc3339()).collect();
    assert_eq!(feed.len(), 2);
    assert_eq!(
        times,
        vec![ts("2026-09-03T09:10:00Z").to_rfc3339(), ts("2026-09-03T09:05:00Z").to_rfc3339()],
        "later ingests land in front of earlier ones"
    );
}

#[test]
fn ingest_orders_a_timestamp_tie_by_project_name() {
    let mk = |id: &str, name: &str, at: &str| {
        with_agent(project(id, name, StatusBucket::Active), "claude-code", "Stop", at)
    };
    let prev = radar_at(
        "2026-09-03T08:00:00Z",
        vec![mk("b", "bravo", "2026-09-03T08:00:00Z"), mk("a", "alpha", "2026-09-03T08:00:00Z")],
    );
    let next = radar_at(
        "2026-09-03T09:00:00Z",
        vec![mk("b", "bravo", "2026-09-03T09:00:00Z"), mk("a", "alpha", "2026-09-03T09:00:00Z")],
    );
    let mut feed = FeedState::default();
    feed.ingest(&prev, &next);
    let names: Vec<&str> = feed.events().iter().map(|e| e.project.as_str()).collect();
    // Same timestamp: the LAST pushed ends up at the front, and rows are pushed in
    // ascending (at, name) order — so bravo, then alpha.
    assert_eq!(names, vec!["bravo", "alpha"], "tie order must not depend on input order");
}

#[test]
fn feed_is_capped_at_feed_capacity_dropping_the_oldest() {
    use petri::feed::FEED_CAPACITY;

    let mut feed = FeedState::default();
    let mut prev = radar_at("2026-09-03T00:00:00Z", vec![project("a", "alpha", StatusBucket::Active)]);

    // Drive one agent event per tick, well past the cap.
    for i in 1..=(FEED_CAPACITY + 25) {
        let at = chrono::DateTime::<chrono::Utc>::from_timestamp(1_800_000_000 + (i as i64) * 60, 0)
            .expect("valid timestamp");
        let mut p = project("a", "alpha", StatusBucket::Active);
        p.agent = AgentState {
            state: AgentActivity::Working,
            active_agent: Some("claude-code".to_string()),
            last_event: Some("Stop".to_string()),
            last_event_at: Some(at),
            session_id: None,
            waiting_since: None,
        };
        let next = Radar {
            schema_version: 1,
            updated_at: at,
            scan_duration_ms: 0,
            projects: vec![p],
            quota: None,
        };
        feed.ingest(&prev, &next);
        prev = next;
    }

    assert_eq!(feed.len(), FEED_CAPACITY, "the queue is bounded");
    let front = feed.events().front().unwrap().at;
    let back = feed.events().back().unwrap().at;
    assert!(front > back, "newest stays at the front after truncation");
    // The oldest surviving row is the (FEED_CAPACITY)th-newest, not the first ever seen.
    assert_eq!(
        back,
        chrono::DateTime::<chrono::Utc>::from_timestamp(
            1_800_000_000 + ((FEED_CAPACITY + 25) as i64 - FEED_CAPACITY as i64 + 1) * 60,
            0
        )
        .unwrap(),
        "truncation drops from the back (oldest), not the front"
    );
}

// ------------------------------------------- orchestrator follow-ups (post-delegation)
//
// Two holes the 24 tests above did not cover, found by reading the delegated diff rather
// than by the gate. Both are asymmetries: one arm of `ingest` handles a case its sibling
// silently drops, and one detail suffix trusts a flag instead of the number it summarises.

#[test]
fn ingest_treats_a_first_ever_commit_as_an_advance() {
    // Mirrors `ingest_treats_a_first_ever_agent_timestamp_as_an_advance`: `None` is older
    // than any `Some` for commits too. A repo landing its first commit while petri is open
    // must produce a row, not silence.
    let mut before = project("a", "alpha", StatusBucket::Active);
    before.git = GitState { is_repo: true, branch: Some("main".to_string()), ..GitState::not_a_repo() };
    let after = with_git(project("a", "alpha", StatusBucket::Active), "main", 0, Some("2026-09-03T09:30:00Z"));

    let mut feed = FeedState::default();
    feed.ingest(
        &radar_at("2026-09-03T09:00:00Z", vec![before]),
        &radar_at("2026-09-03T09:31:00Z", vec![after]),
    );

    assert_eq!(feed.len(), 1, "first-ever commit must emit a row");
    let e = feed.events().front().unwrap();
    assert_eq!(e.kind, FeedKind::Commit);
    assert_eq!(e.at, ts("2026-09-03T09:30:00Z"));
    assert_eq!(e.detail, "commit on main");
}

#[test]
fn agent_detail_suffix_follows_the_count_not_the_dirty_flag() {
    // `is_dirty` and `uncommitted_files` are derived independently by the scanner; if they
    // ever disagree, the row must not read "· 0 files". The count is the thing being
    // reported, so the count is what decides whether the suffix exists at all.
    let mut p = with_agent(project("a", "alpha", StatusBucket::Active), "claude-code", "Stop", "2026-09-03T09:00:00Z");
    p.git = GitState {
        is_repo: true,
        branch: Some("main".to_string()),
        is_dirty: true,
        uncommitted_files: 0,
        ..GitState::not_a_repo()
    };
    assert_eq!(agent_detail(&p), "claude-code stop");
}

// --- MECH-5: the waiting transition as a feed row ---------------------------

#[test]
fn a_new_waiting_latch_emits_one_feed_row() {
    let prev = radar_at("2026-09-04T10:00:00Z", vec![project("p1", "alpha", StatusBucket::Active)]);
    let mut next_p = project("p1", "alpha", StatusBucket::Active);
    next_p.agent.waiting_since = Some(ts("2026-09-04T10:00:30Z"));
    let next = radar_at("2026-09-04T10:01:00Z", vec![next_p]);

    let mut feed = FeedState::default();
    feed.ingest(&prev, &next);

    let rows: Vec<_> = feed.events().iter().filter(|e| e.kind == FeedKind::Waiting).collect();
    assert_eq!(rows.len(), 1, "one row for the transition, got {:?}", feed.events());
    assert_eq!(rows[0].project, "alpha");
    assert_eq!(
        rows[0].at,
        ts("2026-09-04T10:00:30Z"),
        "the row is stamped when the wait began, not when the scan noticed it"
    );
    assert!(rows[0].detail.contains("waiting on you"), "got {:?}", rows[0].detail);
}

#[test]
fn a_carried_forward_latch_does_not_re_emit_every_tick() {
    // The latch is carried forward unchanged for as long as the human takes to answer — the
    // exact period the feed is most likely to be read. An `is_some()` test here would fill
    // the whole block with copies of one event.
    let since = ts("2026-09-04T10:00:30Z");
    let mut p1 = project("p1", "alpha", StatusBucket::Active);
    p1.agent.waiting_since = Some(since);
    let prev = radar_at("2026-09-04T10:01:00Z", vec![p1.clone()]);
    let next = radar_at("2026-09-04T10:02:00Z", vec![p1]);

    let mut feed = FeedState::default();
    feed.ingest(&prev, &next);

    assert!(
        feed.events().iter().all(|e| e.kind != FeedKind::Waiting),
        "an unchanged latch is not news, got {:?}", feed.events()
    );
}

#[test]
fn a_second_wait_after_the_first_was_answered_emits_a_second_row() {
    // Answer, then get asked again: two distinct waits, two rows. This is the case the
    // "don't re-emit" rule above must not over-suppress.
    let mut p_wait1 = project("p1", "alpha", StatusBucket::Active);
    p_wait1.agent.waiting_since = Some(ts("2026-09-04T10:00:30Z"));
    let answered = project("p1", "alpha", StatusBucket::Active); // latch released
    let mut p_wait2 = project("p1", "alpha", StatusBucket::Active);
    p_wait2.agent.waiting_since = Some(ts("2026-09-04T10:05:00Z"));

    let mut feed = FeedState::default();
    feed.ingest(
        &radar_at("2026-09-04T10:00:00Z", vec![project("p1", "alpha", StatusBucket::Active)]),
        &radar_at("2026-09-04T10:01:00Z", vec![p_wait1]),
    );
    feed.ingest(
        &radar_at("2026-09-04T10:01:00Z", vec![answered.clone()]),
        &radar_at("2026-09-04T10:02:00Z", vec![answered]),
    );
    feed.ingest(
        &radar_at("2026-09-04T10:04:00Z", vec![project("p1", "alpha", StatusBucket::Active)]),
        &radar_at("2026-09-04T10:05:30Z", vec![p_wait2]),
    );

    let rows: Vec<_> = feed.events().iter().filter(|e| e.kind == FeedKind::Waiting).collect();
    assert_eq!(rows.len(), 2, "two separate waits -> two rows, got {:?}", feed.events());
}
