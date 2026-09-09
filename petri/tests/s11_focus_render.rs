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
//!
//! ## One test per rung, deliberately
//!
//! The rungs are split across Phase B tasks (T1b owns R0-R3 and the empty states, T2 owns
//! R4 and R7, T3 owns R5, T4 owns R6) and §2's keep rule reverts any round that does not
//! *reduce* the failing count. A test asserting six rungs at once would therefore score zero
//! for five of those six tasks and get their correct work thrown away. So: one rung, one
//! test, and no test depends on a rung another task owns.

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

/// A rect that plans every rung except `Repo`/`Tree` — the size most tests below use, so a
/// rung's absence is never an artefact of the geometry.
const ROOMY: (u16, u16) = (96, 26);

/// A rect that plans the entire ladder, for the two rungs `ROOMY` cannot reach.
const LUSH: (u16, u16) = (116, 36);

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

    /// The zone row whose label is `label` — `git`, `agent`, `last`, `repo`. The mockups
    /// (`PROPOSAL-focus-panel.md` §3.1-§3.3) all put the zone label first on its own row,
    /// after the indent, and that IS the layout being specified here: it is what lets the
    /// eye scan a column of labels rather than hunt for facts.
    fn zone(&self, label: &str) -> &str {
        self.rows
            .iter()
            .find(|r| r.trim_start().starts_with(label))
            .unwrap_or_else(|| {
                panic!(
                    "no rendered row starts with the zone label {label:?}:\n{}",
                    self.text()
                )
            })
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

    fn render(&self, target: FocusTarget, (w, h): (u16, u16)) -> Rendered {
        Rendered::of(&focus_lines(Rect::new(0, 0, w, h), &self.ctx(target)))
    }

    fn render_project(&self, name: &str, size: (u16, u16)) -> Rendered {
        self.render(FocusTarget::Project(idx_of(&self.radar, name)), size)
    }

    fn plan(&self, target: FocusTarget, (w, h): (u16, u16)) -> Vec<Rung> {
        plan_rungs(Rect::new(0, 0, w, h), &self.ctx(target))
    }

    fn plan_project(&self, name: &str, size: (u16, u16)) -> Vec<Rung> {
        self.plan(FocusTarget::Project(idx_of(&self.radar, name)), size)
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
// R0-R3 — identity, path, git, agent (T1b).
// ---------------------------------------------------------------------------

#[test]
fn r0_identity_names_the_project_and_marks_its_uncommitted_files() {
    let c = Case::seeded("normal.json");
    let r = c.render_project("alpha-project", ROOMY);

    r.assert_has(
        "alpha-project",
        "R0 says which project the panel is pointed at",
    );
    let row = r.row_with("alpha-project");
    assert!(
        row.contains("✎2"),
        "alpha-project has uncommitted_files: 2, and §2's R0 renders that as `✎N` — a bare \
         2 somewhere on the panel would not be the same claim: got {row:?}"
    );
}

#[test]
fn r1_path_shows_the_project_directory() {
    let c = Case::seeded("normal.json");
    let r = c.render_project("alpha-project", ROOMY);
    r.assert_has(
        "/repos/JKrag/alpha-project",
        "R1 is the path — `~`-abbreviated when it is under $HOME, which the fixture's \
         `/Users/jankrag/...` is not on every machine, so only the tail is pinned",
    );
}

#[test]
fn r2_git_pairs_the_branch_with_the_commit_age() {
    let c = Case::seeded("normal.json");
    let r = c.render_project("alpha-project", ROOMY);
    let row = r.zone("git");
    assert!(row.contains("main"), "the branch: got {row:?}");
    assert!(
        row.contains("18h"),
        "last_commit_at is 2026-08-19T14:22Z and now is pinned to 2026-08-20T09:15Z, so \
         `commit_ago` reads 18h — reusing dashboard.rs's helper rather than inventing a \
         second age format: got {row:?}"
    );
}

#[test]
fn r3_agent_names_the_agent_and_its_session() {
    let c = Case::seeded("normal.json");
    let r = c.render_project("alpha-project", ROOMY);
    let row = r.zone("agent");
    assert!(row.contains("claude-code"), "the agent: got {row:?}");
    assert!(
        row.contains("a1b2c3d4"),
        "the session id, truncated as the roomy card truncates it: got {row:?}"
    );
}

#[test]
fn r3_agent_says_idle_rather_than_going_blank_with_no_agent() {
    // `iron-depot` has `active_agent: null`. The rung is still planned (it is unconditional
    // above the floor), so it must say something true rather than render an empty label.
    let c = Case::seeded("hostile.json");
    let r = c.render_project("iron-depot", ROOMY);
    let row = r.zone("agent");
    assert!(
        row.trim().len() > "agent".len(),
        "an agent-less project still gets a populated agent row: got {row:?}"
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
    let r = c.render(FocusTarget::Project(0), ROOMY);

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
    let r = c.render(FocusTarget::Project(0), ROOMY);

    r.assert_lacks(
        "waiting",
        "a latch older than WAITING_MAX_LATCH_S has expired — reading \
         `waiting_since.is_some()` directly pins a dead ▲ forever",
    );
}

// ---------------------------------------------------------------------------
// The states that are not a project (T1b — `focus.rs`-only, so it stays
// conflict-free with the rung tasks, and Phase B can reach zero without the
// Phase D mounts).
// ---------------------------------------------------------------------------

#[test]
fn a_section_target_names_the_section_and_its_count() {
    let c = Case::seeded("normal.json");
    let label = SECTION_LABELS
        .iter()
        .find(|(b, _)| *b == StatusBucket::Active)
        .map(|(_, l)| *l)
        .expect("SECTION_LABELS covers Active");
    let r = c.render(FocusTarget::Section(StatusBucket::Active, 4), ROOMY);

    r.assert_has(
        label,
        "PROPOSAL §3.1 option (b): a header stop renders a rung-zero state naming the \
         section, so navigation semantics stay identical whether the popup is open or not",
    );
    r.assert_has(
        "4",
        "…and how many projects are in it, so the row says something the header did not",
    );
    r.assert_lacks(
        "alpha-project",
        "no project is focused, so no project's facts may be shown",
    );
}

#[test]
fn an_empty_target_renders_without_panicking_and_names_no_project() {
    let c = Case::seeded("normal.json");
    let r = c.render(FocusTarget::Nothing, ROOMY);
    r.assert_lacks("alpha-project", "nothing is focused");
    assert!(
        r.rows.len() <= ROOMY.1 as usize,
        "the empty state still respects the rect it was handed"
    );
}

// ---------------------------------------------------------------------------
// R4 + R7 — last event, repo (T2).
// ---------------------------------------------------------------------------

#[test]
fn r4_last_event_shows_what_the_agent_did_and_when() {
    let c = Case::seeded("normal.json");
    let r = c.render_project("alpha-project", ROOMY);
    let row = r.zone("last");
    assert!(
        row.contains("09:14"),
        "`last_event_at` is 2026-08-20T09:14:30Z — the clock, at minute precision, from \
         the pinned now: got {row:?}"
    );
    assert!(
        row.contains("tool"),
        "`last_event` is \"PreToolUse\", which `feed::humanize_event` renders as \
         \"pre tool use\" — the humaniser is reused, not re-implemented: got {row:?}"
    );
}

#[test]
fn r4_falls_back_when_the_sensor_named_no_event() {
    // `agent.last_event: None` is legitimate, not a defect: `swab` derives event names from
    // an allowlist and an unmodelled record type yields None ON PURPOSE. The row falls back
    // to `feed::agent_detail`, so "claude-code activity" is correct output — do NOT widen
    // the allowlist in `swab` (protected) to make this go away.
    let mut p = project("quiet", StatusBucket::Active);
    p.agent.active_agent = Some("claude-code".to_string());
    p.agent.last_event = None;
    p.agent.last_event_at = Some(pinned_now() - chrono::Duration::minutes(3));
    let c = Case::of_radar(radar_of(vec![p]));
    let r = c.render(FocusTarget::Project(0), ROOMY);

    let row = r.zone("last");
    assert!(
        row.contains("claude-code") && row.contains("activity"),
        "`agent_detail`'s fallback, verbatim: got {row:?}"
    );
}

#[test]
fn r7_repo_shows_the_url_and_does_not_repeat_one_age_twice() {
    let c = Case::seeded("normal.json");
    // LUSH is the only size in this file that plans Repo — assert that, so the test cannot
    // quietly stop testing Repo if the gates move.
    assert!(
        c.plan_project("alpha-project", LUSH).contains(&Rung::Repo),
        "precondition: Repo is planned at {LUSH:?}"
    );

    let r = c.render_project("alpha-project", LUSH);
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
}

#[test]
fn r7_repo_calls_out_a_commit_that_is_not_yours() {
    // brisbane-engine's `mine_last_commit_at` (08-18) and `last_commit_at` (08-19) differ —
    // the "someone else pushed here" signal, which is the whole reason this rung exists.
    let c = Case::seeded("normal.json");
    let r = c.render_project("brisbane-engine", LUSH);
    let row = r.row_with("github.com/JKrag/brisbane-engine");
    assert!(
        row.contains("newest"),
        "the two ages diverge, so both are shown and labelled: got {row:?}"
    );
}

// ---------------------------------------------------------------------------
// R5 — actions as affordances (T3).
// ---------------------------------------------------------------------------

#[test]
fn r5_actions_are_labelled_and_offer_their_keys() {
    let c = Case::seeded("normal.json");
    let r = c.render_project("alpha-project", ROOMY);
    r.assert_has("ACTIONS", "§9's dim-uppercase section label, no sub-border");

    let label_at = r
        .rows
        .iter()
        .position(|row| row.contains("ACTIONS"))
        .expect("asserted above");
    let entries = r
        .rows
        .iter()
        .skip(label_at + 1)
        .find(|row| !row.trim().is_empty())
        .unwrap_or_else(|| panic!("the ACTIONS label has no entries under it:\n{}", r.text()));
    let mut chars = entries.trim_start().chars();
    let key = chars.next().expect("non-empty row");
    assert!(
        key.is_ascii_alphanumeric() && chars.next() == Some(' '),
        "each entry leads with the key itself — information rather than texture, per §9's \
         always-on-markers audit (no ▶, no bullets): got {entries:?}"
    );
}

#[test]
fn r5_a_missing_target_is_disabled_in_words_not_only_in_colour() {
    // `deltaflow` has `github_url: null`, so the remote action resolves to
    // `Resolution::NoTarget` — SPEC.md §5 forbids advertising a key that does nothing, and
    // §4's table makes that a *dimmed, annotated* entry rather than an omitted one (an
    // omission is reserved for `NoTool`, where the key genuinely cannot work at all).
    let c = Case::seeded("normal.json");
    let r = c.render_project("deltaflow", ROOMY);
    r.assert_has(
        "no url",
        "the reason the key is inert, in words — dimming alone fails NO_COLOR and fails \
         CVD readers",
    );
    assert!(
        r.row_with("no url").contains('─'),
        "…and a glyph beside the words, per §4's non-colour fallback: got {:?}",
        r.row_with("no url")
    );
}

#[test]
fn r5_git_history_is_disabled_in_a_project_that_is_not_a_repo() {
    // Issue #38. `lunar-fetcher` in `hostile.json` has `is_repo: false` — a directory
    // discovery admitted on a manifest rather than a `.git`, which is a legitimate fleet
    // member. `g` there used to launch git and have it exit immediately, flashing the
    // screen; it is now the same dimmed, annotated entry `o` gets with no remote.
    let c = Case::seeded("hostile.json");
    let r = c.render_project("lunar-fetcher", ROOMY);
    r.assert_has(
        "not a repo",
        "the reason `g` is inert, in words rather than by dimming alone",
    );
    assert!(
        r.row_with("not a repo").contains('─'),
        "…with §4's non-colour fallback glyph: got {:?}",
        r.row_with("not a repo")
    );
}

// ---------------------------------------------------------------------------
// R6 — the per-project recent slice (T4).
// ---------------------------------------------------------------------------

#[test]
fn r6_recent_is_labelled_and_shows_this_projects_events() {
    let c = Case::seeded("normal.json");
    let r = c.render_project("alpha-project", ROOMY);
    r.assert_has("RECENT", "§9's dim-uppercase section label");
    r.assert_has(
        "09:14",
        "`FeedState::seeded` gives alpha-project a row at its `last_event_at`, and \
         `FeedEvent::stamp` renders today's events as a clock",
    );
}

#[test]
fn r6_recent_is_filtered_to_the_focused_project() {
    let c = Case::seeded("normal.json");
    let r = c.render_project("brisbane-engine", ROOMY);
    for other in ["alpha-project", "catalyst-ui", "deltaflow"] {
        r.assert_lacks(
            other,
            "the feed is fleet-wide; this rung is the slice belonging to the focused \
             project, so no other project's name may appear anywhere on the panel",
        );
    }
}

// ---------------------------------------------------------------------------
// Render follows the plan, at every size. One assertion per rung, for the
// same keep-rule reason the rung tests are split.
// ---------------------------------------------------------------------------

/// Sizes spanning every gate boundary the labelled rungs cross.
const LADDER_SIZES: [(u16, u16); 7] = [LUSH, ROOMY, (76, 20), (40, 15), (44, 10), (30, 7), (24, 6)];

#[test]
fn the_actions_label_appears_exactly_when_there_is_room_for_it() {
    let c = Case::seeded("normal.json");
    for size in LADDER_SIZES {
        let plan = c.plan_project("alpha-project", size);
        let r = c.render_project("alpha-project", size);
        // Below 16 rows the label itself IS the degradation (§3.3: bare keys), which is
        // exactly the difference between Actions' 3-row and 1-row costs.
        let expects_label = plan.contains(&Rung::Actions) && size.1 >= 16;
        assert_eq!(
            r.contains("ACTIONS"),
            expects_label,
            "{size:?}: plan is {plan:?}, so the ACTIONS label should be \
             present={expects_label}. Got:\n{}",
            r.text()
        );
    }
}

#[test]
fn the_recent_label_appears_exactly_when_recent_is_planned() {
    let c = Case::seeded("normal.json");
    for size in LADDER_SIZES {
        let plan = c.plan_project("alpha-project", size);
        let r = c.render_project("alpha-project", size);
        // Unlike Actions, Recent has no reduced form: its 3 rows ARE the label plus two
        // event rows, so a planned Recent always carries its label.
        assert_eq!(
            r.contains("RECENT"),
            plan.contains(&Rung::Recent),
            "{size:?}: plan is {plan:?}. Got:\n{}",
            r.text()
        );
    }
}

#[test]
fn the_path_rung_appears_exactly_when_it_is_planned() {
    let c = Case::seeded("normal.json");
    for size in [ROOMY, (44, 10), (44, 9), (30, 7)] {
        let plan = c.plan_project("alpha-project", size);
        let r = c.render_project("alpha-project", size);
        assert_eq!(
            r.contains("/repos/JKrag/alpha-project"),
            plan.contains(&Rung::Path),
            "{size:?}: the path row must track the plan exactly (plan is {plan:?}). Got:\n{}",
            r.text()
        );
    }
}

#[test]
fn below_the_floor_renders_nothing_at_all() {
    let c = Case::seeded("normal.json");
    for size in [(23, 40), (200, 5), (10, 3)] {
        let r = c.render_project("alpha-project", size);
        assert!(
            r.rows.is_empty(),
            "{size:?} is below the floor: the panel renders nothing and the MOUNT owns the \
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
            for size in [
                (24u16, 6u16),
                (30, 7),
                (40, 15),
                (44, 10),
                (76, 20),
                ROOMY,
                LUSH,
                (240, 60),
            ] {
                let (w, h) = size;
                let r = c.render(FocusTarget::Project(idx), size);
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
        for size in [(24u16, 6u16), (44, 10), ROOMY, (240, 60)] {
            let r = c.render(target.clone(), size);
            assert!(
                r.rows.len() <= size.1 as usize,
                "{target:?} at {size:?} rendered {} rows",
                r.rows.len()
            );
        }
    }
}
