//! `SURF-8` layer 2 — what the focus panel's rungs actually say (`focus::focus_lines`).
//!
//! **Protected. Authored by the planner, not the implementer** (`PLAN-focus-panel.md` §1 and
//! §3). `focus::focus_lines` is `unimplemented!()` in the Phase A scaffold, so every test
//! here FAILS (panics) rather than errors.
//!
//! ## Structural, not byte-for-byte
//!
//! Per `SPEC.md` §8's rule for this layer: assert the *content that must appear*, not an
//! exact frame. Spacing, column positions and styling are the implementer's to choose; which
//! facts reach the screen is not. Two consequences worth stating, because they are what
//! makes a test here worth writing:
//!
//! - Where a rung's presence is decided by `plan_rungs`, the assertion is tied back to
//!   `plan_rungs` rather than to a hard-coded size, so the two cannot drift apart.
//! - `now` is pinned. Every silence string, commit age and clock is derived at render time,
//!   so a live `Utc::now()` would make these assertions depend on the calendar — the exact
//!   shape of the time-bomb `PLAN-focus-panel.md` §10 records biting `swab`'s quota tests.

use chrono::{DateTime, TimeZone, Utc};
use petri::dashboard::SECTION_LABELS;
use petri::feed::FeedState;
use petri::focus::{FocusCtx, FocusTarget, Rung, focus_lines, plan_rungs};
use petri::prefs::Prefs;
use petridish_core::schema::{AgentState, GitState, Project, Radar, StatusBucket};
use ratatui::layout::Rect;
use ratatui::text::Line;
use std::path::PathBuf;
use unicode_width::UnicodeWidthStr;

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

/// See `s11_focus_plan.rs` — `normal.json` was captured at `2026-08-20T09:15:00Z`.
fn pinned_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 20, 9, 15, 0).unwrap()
}

fn idx_of(radar: &Radar, name: &str) -> usize {
    radar
        .projects
        .iter()
        .position(|p| p.name == name)
        .unwrap_or_else(|| panic!("fixture has no project named {name}"))
}

/// A rendered panel, flattened to plain text one line per row — the "screen grid" this
/// layer asserts against.
struct Rendered {
    rows: Vec<String>,
}

impl Rendered {
    fn of(lines: &[Line<'static>]) -> Self {
        Rendered {
            rows: lines
                .iter()
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                })
                .collect(),
        }
    }

    fn text(&self) -> String {
        self.rows.join("\n")
    }

    fn contains(&self, needle: &str) -> bool {
        self.rows.iter().any(|r| r.contains(needle))
    }

    /// The one row containing `needle`, for assertions about what else is (or is not) on
    /// that same row.
    fn row_with(&self, needle: &str) -> &str {
        self.rows
            .iter()
            .find(|r| r.contains(needle))
            .unwrap_or_else(|| panic!("no rendered row contains {needle:?}:\n{}", self.text()))
            .as_str()
    }

    fn assert_has(&self, needle: &str, why: &str) {
        assert!(
            self.contains(needle),
            "expected a row containing {needle:?} ({why}), got:\n{}",
            self.text()
        );
    }

    fn assert_lacks(&self, needle: &str, why: &str) {
        assert!(
            !self.contains(needle),
            "expected NO row containing {needle:?} ({why}), got:\n{}",
            self.text()
        );
    }
}

struct Case {
    radar: Radar,
    feed: Option<FeedState>,
    prefs: Prefs,
}

impl Case {
    fn seeded(fixture: &str) -> Self {
        let radar = load(fixture);
        let feed = FeedState::seeded(&radar);
        Case {
            radar,
            feed: Some(feed),
            prefs: Prefs::default(),
        }
    }

    fn of_radar(radar: Radar) -> Self {
        Case {
            radar,
            feed: None,
            prefs: Prefs::default(),
        }
    }

    fn ctx(&self, target: FocusTarget) -> FocusCtx<'_> {
        FocusCtx {
            radar: &self.radar,
            target,
            now: pinned_now(),
            feed: self.feed.as_ref(),
            prefs: &self.prefs,
        }
    }

    fn render(&self, target: FocusTarget, w: u16, h: u16) -> Rendered {
        Rendered::of(&focus_lines(Rect::new(0, 0, w, h), &self.ctx(target)))
    }

    fn render_project(&self, name: &str, w: u16, h: u16) -> Rendered {
        self.render(FocusTarget::Project(idx_of(&self.radar, name)), w, h)
    }

    fn plan(&self, target: FocusTarget, w: u16, h: u16) -> Vec<Rung> {
        plan_rungs(Rect::new(0, 0, w, h), &self.ctx(target))
    }
}

/// A synthetic project, so a test about one field is not hostage to a fixture happening to
/// carry the right shape. Same constructor idiom as `s6_snapshot.rs`.
fn project(name: &str, bucket: StatusBucket) -> Project {
    Project {
        id: format!("id-{name}"),
        name: name.to_string(),
        path: format!("/repos/{name}"),
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
        updated_at: pinned_now(),
        scan_duration_ms: 0,
        projects,
        quota: None,
    }
}

// ---------------------------------------------------------------------------
// The rungs say what they are for.
// ---------------------------------------------------------------------------

#[test]
fn a_roomy_panel_shows_every_rung_it_planned() {
    let c = Case::seeded("normal.json");
    let r = c.render_project("alpha-project", 96, 26);

    r.assert_has("alpha-project", "R0 identity names the project");
    r.assert_has("/repos/JKrag/alpha-project", "R1 is the path");
    r.assert_has("main", "R2 carries the branch");
    r.assert_has("claude-code", "R3 names the agent");
    r.assert_has("09:14", "R4 is the last event's clock, from the pinned now");
    r.assert_has("ACTIONS", "R5's section label");
    r.assert_has("RECENT", "R6's section label");
}

#[test]
fn the_identity_rung_carries_the_uncommitted_count() {
    // alpha-project has `uncommitted_files: 2` — the `✎N` marker of §2's R0 row.
    let c = Case::seeded("normal.json");
    let r = c.render_project("alpha-project", 96, 26);
    r.assert_has("2", "the uncommitted-files count reaches the identity rung");
    assert!(
        r.row_with("alpha-project").contains('✎'),
        "the dirty count is marked with ✎ on the identity row, not left as a bare number: \
         got {:?}",
        r.row_with("alpha-project")
    );
}

#[test]
fn the_repo_rung_shows_the_url_and_does_not_repeat_one_age_twice() {
    let c = Case::seeded("normal.json");
    // 116x36 is the only size in this file that plans Repo — assert that, so the test
    // cannot quietly stop testing Repo if the gates move.
    let plan = c.plan(
        FocusTarget::Project(idx_of(&c.radar, "alpha-project")),
        116,
        36,
    );
    assert!(plan.contains(&Rung::Repo), "precondition: Repo is planned");

    let r = c.render_project("alpha-project", 116, 36);
    r.assert_has(
        "github.com/JKrag/alpha-project",
        "R7 surfaces the github url, which nothing else in the UI shows",
    );
    // alpha-project's `mine_last_commit_at == last_commit_at` — the common case. One age,
    // not two identical ones (PLAN §5 T2).
    let row = r.row_with("github.com/JKrag/alpha-project");
    assert!(
        !row.contains("newest"),
        "yours == newest, so the divergence half of the row must not be rendered at all: \
         got {row:?}"
    );

    // brisbane-engine's differ by a day — there the divergence IS the point of the row.
    let r = c.render_project("brisbane-engine", 116, 36);
    let row = r.row_with("github.com/JKrag/brisbane-engine");
    assert!(
        row.contains("newest"),
        "mine_last_commit_at != last_commit_at is the 'someone else pushed here' signal \
         this rung exists for: got {row:?}"
    );
}

// ---------------------------------------------------------------------------
// The waiting latch, re-derived at render time (PLAN §5 T1b's gotcha, SPEC §3.2).
// ---------------------------------------------------------------------------

#[test]
fn a_live_waiting_latch_is_announced_on_the_identity_rung() {
    let mut p = project("blocked", StatusBucket::Active);
    p.agent.waiting_since = Some(pinned_now() - chrono::Duration::minutes(4));
    let c = Case::of_radar(radar_of(vec![p]));
    let r = c.render(FocusTarget::Project(0), 96, 26);

    r.assert_has(
        "waiting",
        "MECH-5 is the one state the whole product exists to surface",
    );
    assert!(
        r.row_with("waiting").contains('▲'),
        "glyph, colour AND words — the panel's deliberate three-signal maximum, so the \
         state survives NO_COLOR: got {:?}",
        r.row_with("waiting")
    );
}

#[test]
fn an_expired_waiting_latch_is_not_announced() {
    // Past `WAITING_MAX_LATCH_S` (3h): the scanner stamped it, the daemon then stopped
    // updating, and `petri` must not keep a dead ▲ pinned forever. This is why the latch is
    // re-derived through `waiting_latch_live(.., ctx.now)` rather than read off the field.
    let mut p = project("stale-latch", StatusBucket::Active);
    p.agent.waiting_since = Some(pinned_now() - chrono::Duration::hours(4));
    let c = Case::of_radar(radar_of(vec![p]));
    let r = c.render(FocusTarget::Project(0), 96, 26);

    r.assert_lacks(
        "waiting",
        "a latch older than WAITING_MAX_LATCH_S has expired — reading \
         `waiting_since.is_some()` directly pins a dead ▲ forever",
    );
}

// ---------------------------------------------------------------------------
// Render follows the plan, at every size.
// ---------------------------------------------------------------------------

/// The section labels a rung owns, for the rungs whose presence is visible as a label.
fn label_for(rung: Rung) -> Option<&'static str> {
    match rung {
        Rung::Actions => Some("ACTIONS"),
        Rung::Recent => Some("RECENT"),
        _ => None,
    }
}

#[test]
fn a_labelled_rung_appears_exactly_when_it_is_planned() {
    let c = Case::seeded("normal.json");
    let idx = idx_of(&c.radar, "alpha-project");
    for (w, h) in [(116, 36), (96, 26), (76, 20), (44, 10), (30, 7), (24, 6)] {
        let plan = c.plan(FocusTarget::Project(idx), w, h);
        let r = c.render(FocusTarget::Project(idx), w, h);
        for rung in [Rung::Actions, Rung::Recent] {
            let Some(label) = label_for(rung) else {
                continue;
            };
            // Below 16 rows the ACTIONS label itself is the degradation (§3.3: bare keys),
            // so only assert the label where the rung is planned at its full 3-row cost.
            let expects_label = plan.contains(&rung) && h >= 16;
            assert_eq!(
                r.contains(label),
                expects_label,
                "{w}x{h}: plan is {plan:?}, so {label} should be present={expects_label}. \
                 Got:\n{}",
                r.text()
            );
        }
    }
}

#[test]
fn the_path_rung_appears_exactly_when_it_is_planned() {
    let c = Case::seeded("normal.json");
    let idx = idx_of(&c.radar, "alpha-project");
    for (w, h) in [(96, 26), (44, 10), (44, 9), (30, 7)] {
        let plan = c.plan(FocusTarget::Project(idx), w, h);
        let r = c.render(FocusTarget::Project(idx), w, h);
        assert_eq!(
            r.contains("/repos/JKrag/alpha-project"),
            plan.contains(&Rung::Path),
            "{w}x{h}: the path row must track the plan exactly (plan is {plan:?}). Got:\n{}",
            r.text()
        );
    }
}

// ---------------------------------------------------------------------------
// The states that are not a project.
// ---------------------------------------------------------------------------

#[test]
fn a_section_target_names_the_section_and_its_count() {
    let c = Case::seeded("normal.json");
    let label = SECTION_LABELS
        .iter()
        .find(|(b, _)| *b == StatusBucket::Active)
        .map(|(_, l)| *l)
        .expect("SECTION_LABELS covers Active");
    let r = c.render(FocusTarget::Section(StatusBucket::Active, 4), 96, 26);

    r.assert_has(
        label,
        "PROPOSAL §3.1 option (b): a header stop renders a rung-zero state naming the \
         section, so navigation semantics stay identical whether the popup is open or not",
    );
    r.assert_has("4", "…and how many projects are in it");
    r.assert_lacks(
        "alpha-project",
        "no project is focused, so no project's facts may be shown",
    );
}

#[test]
fn an_empty_target_renders_without_panicking_and_names_no_project() {
    let c = Case::seeded("normal.json");
    let r = c.render(FocusTarget::Nothing, 96, 26);
    r.assert_lacks("alpha-project", "nothing is focused");
    assert!(
        r.rows.len() <= 26,
        "the empty state still respects the rect it was handed"
    );
}

#[test]
fn below_the_floor_renders_nothing_at_all() {
    let c = Case::seeded("normal.json");
    for (w, h) in [(23, 40), (200, 5), (10, 3)] {
        let r = c.render_project("alpha-project", w, h);
        assert!(
            r.rows.is_empty(),
            "{w}x{h} is below the floor: the panel renders nothing and the MOUNT owns the \
             'needs 24x6' message (PROPOSAL §3.5), because only the mount knows which of \
             the two wordings applies. Got:\n{}",
            r.text()
        );
    }
}

// ---------------------------------------------------------------------------
// The net: no size, no fixture, no target may overflow or panic.
// ---------------------------------------------------------------------------

#[test]
fn no_rendered_line_ever_overflows_its_rect() {
    // `hostile.json` is the point of this test: a CJK project name (columns != characters)
    // and a 190-character name, both of which must be measured with the same width table
    // ratatui's own cell maths uses.
    for fixture in ["normal.json", "hostile.json", "minimal.json", "loaded.json"] {
        let c = Case::seeded(fixture);
        for idx in 0..c.radar.projects.len() {
            for (w, h) in [
                (24, 6),
                (30, 7),
                (40, 15),
                (44, 10),
                (76, 20),
                (96, 26),
                (116, 36),
                (240, 60),
            ] {
                let r = c.render(FocusTarget::Project(idx), w, h);
                assert!(
                    r.rows.len() <= h as usize,
                    "{fixture} project {idx} at {w}x{h}: rendered {} rows",
                    r.rows.len()
                );
                for (i, row) in r.rows.iter().enumerate() {
                    assert!(
                        row.width() <= w as usize,
                        "{fixture} project {idx} at {w}x{h}: row {i} is {} columns wide — \
                         {row:?}",
                        row.width()
                    );
                }
            }
        }
    }
}

#[test]
fn every_target_kind_renders_at_every_size_without_panicking() {
    let c = Case::seeded("hostile.json");
    let mut targets = vec![FocusTarget::Nothing];
    for (bucket, _) in SECTION_LABELS {
        targets.push(FocusTarget::Section(bucket, 0));
        targets.push(FocusTarget::Section(bucket, 999));
    }
    for target in targets {
        for (w, h) in [(24, 6), (44, 10), (96, 26), (240, 60)] {
            let r = c.render(target.clone(), w, h);
            assert!(
                r.rows.len() <= h as usize,
                "{target:?} at {w}x{h} rendered {} rows",
                r.rows.len()
            );
        }
    }
}
