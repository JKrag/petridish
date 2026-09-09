//! `SURF-8` layer 1 — the focus panel's responsive arithmetic (`focus::plan_rungs`).
//!
//! **Protected. Authored by the planner, not the implementer** (`PLAN-focus-panel.md` §1 and
//! §3): these tests are the spec for T1a, they were written before any implementation
//! existed, and a round that changes or deletes one is an automatic revert regardless of
//! what the failing-test count does.
//!
//! `focus::plan_rungs`'s body is `unimplemented!()` in the Phase A scaffold, so every test
//! here FAILS (panics) rather than errors — confirmed before delegating Phase B, same
//! convention `s6_snapshot.rs` records for `dashboard::render`'s `todo!()`.
//!
//! ## What the sizes below are, and what they are not
//!
//! Every `Rect` here is a **panel content rect**: the mount has already subtracted its own
//! chrome (a popup's border; `--mini`'s header, rule and footer). They are *not*
//! `PROPOSAL-focus-panel.md` §10's terminal sizes, and no test here claims a mapping between
//! the two. That mapping lives in the mounts (T5's ≤80%-of-terminal popup rule, T7's
//! `--mini` chrome), neither of which exists yet — pinning a guessed chrome height here
//! would hand those tasks a contradiction they are forbidden to resolve. The §10 pressure
//! table gets its own gate in Phase C, against the real mount geometry.
//!
//! The thresholds asserted below are the ones documented on `plan_rungs`. They were fitted
//! to §10 and §3.2–§3.4 rather than derived, which is exactly why they are pinned here: they
//! are a design decision, and the only thing that can hold a design decision still is a test.

use chrono::{DateTime, TimeZone, Utc};
use petri::feed::FeedState;
use petri::focus::{FocusCtx, FocusTarget, Rung, plan_rungs};
use petri::prefs::Prefs;
use petridish_core::schema::{Radar, StatusBucket};
use ratatui::layout::Rect;
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

/// Pinned clock. `normal.json` was captured at `2026-08-20T09:15:00Z`, so this is "the
/// moment of the scan" — a live `Utc::now()` would make the silence-derived strings drift
/// and would make this suite's own failures depend on the calendar (`PLAN-focus-panel.md`
/// §10 records exactly that bug biting `swab`'s quota tests).
fn pinned_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 20, 9, 15, 0).unwrap()
}

/// Index of a project by name, so a test never hard-codes a position into
/// `radar.projects` — the scanner re-sorts on every scan (`SPEC.md` §4.3) and a fixture is
/// just a captured scan.
fn idx_of(radar: &Radar, name: &str) -> usize {
    radar
        .projects
        .iter()
        .position(|p| p.name == name)
        .unwrap_or_else(|| panic!("fixture has no project named {name}"))
}

/// The full ladder in render order — what `plan_rungs` returns a subsequence of.
const LADDER: [Rung; 9] = [
    Rung::Identity,
    Rung::Path,
    Rung::Git,
    Rung::Agent,
    Rung::LastEvent,
    Rung::Actions,
    Rung::Recent,
    Rung::Repo,
    Rung::Tree,
];

/// Minimum rows a rung costs when rendered, per `plan_rungs`' documented table. Used only by
/// the "everything planned still fits" invariant.
fn rung_rows(rung: Rung, height: u16) -> u16 {
    match rung {
        Rung::Actions => {
            if height >= 16 {
                3
            } else {
                1
            }
        }
        Rung::Recent => 3,
        Rung::Tree => 2,
        _ => 1,
    }
}

struct Case {
    radar: Radar,
    feed: Option<FeedState>,
    prefs: Prefs,
}

impl Case {
    /// A project focused with a feed that has seen it — the ordinary case, and the only one
    /// in which `Recent` can appear at all.
    fn seeded(fixture: &str) -> Self {
        let radar = load(fixture);
        let feed = FeedState::seeded(&radar);
        Case {
            radar,
            feed: Some(feed),
            prefs: Prefs::default(),
        }
    }

    fn without_feed(fixture: &str) -> Self {
        Case {
            radar: load(fixture),
            feed: None,
            prefs: Prefs::default(),
        }
    }

    fn with_empty_feed(fixture: &str) -> Self {
        Case {
            radar: load(fixture),
            feed: Some(FeedState::default()),
            prefs: Prefs::default(),
        }
    }

    fn plan(&self, target: FocusTarget, w: u16, h: u16) -> Vec<Rung> {
        let ctx = FocusCtx {
            radar: &self.radar,
            target,
            now: pinned_now(),
            feed: self.feed.as_ref(),
            prefs: &self.prefs,
        };
        plan_rungs(Rect::new(0, 0, w, h), &ctx)
    }

    fn plan_project(&self, name: &str, w: u16, h: u16) -> Vec<Rung> {
        let target = FocusTarget::Project(idx_of(&self.radar, name));
        self.plan(target, w, h)
    }
}

// ---------------------------------------------------------------------------
// The ladder, size by size. `alpha-project` is `normal.json`'s canonical happy
// case: an active claude-code session, a dirty repo with a branch and a github
// url, and an `agent.last_event_at` — so every rung has something to say and the
// only thing deciding the answer is the geometry.
// ---------------------------------------------------------------------------

#[test]
fn a_roomy_panel_gets_the_whole_ladder() {
    let c = Case::seeded("normal.json");
    assert_eq!(
        c.plan_project("alpha-project", 116, 36),
        LADDER.to_vec(),
        "116x36 clears every gate, so the plan is the full ladder in render order"
    );
}

#[test]
fn tree_is_the_first_rung_to_go() {
    let c = Case::seeded("normal.json");
    assert_eq!(
        c.plan_project("alpha-project", 116, 30),
        vec![
            Rung::Identity,
            Rung::Path,
            Rung::Git,
            Rung::Agent,
            Rung::LastEvent,
            Rung::Actions,
            Rung::Recent,
            Rung::Repo,
        ],
        "30 rows is below Tree's 32-row gate but above Repo's 28"
    );
}

#[test]
fn repo_and_tree_both_drop_by_twenty_six_rows() {
    let c = Case::seeded("normal.json");
    assert_eq!(
        c.plan_project("alpha-project", 96, 26),
        vec![
            Rung::Identity,
            Rung::Path,
            Rung::Git,
            Rung::Agent,
            Rung::LastEvent,
            Rung::Actions,
            Rung::Recent,
        ],
    );
}

#[test]
fn a_narrow_panel_drops_repo_and_tree_on_width_alone() {
    let c = Case::seeded("normal.json");
    assert_eq!(
        c.plan_project("alpha-project", 48, 36),
        vec![
            Rung::Identity,
            Rung::Path,
            Rung::Git,
            Rung::Agent,
            Rung::LastEvent,
            Rung::Actions,
            Rung::Recent,
        ],
        "48 columns has all the rows in the world and still cannot hold Repo (56) — the \
         gates are two-dimensional, not a row budget with a width footnote"
    );
}

#[test]
fn recent_survives_down_to_fifteen_rows_and_forty_columns() {
    let c = Case::seeded("normal.json");
    let with_recent = vec![
        Rung::Identity,
        Rung::Path,
        Rung::Git,
        Rung::Agent,
        Rung::LastEvent,
        Rung::Actions,
        Rung::Recent,
    ];
    assert_eq!(c.plan_project("alpha-project", 40, 15), with_recent);

    let without_recent = vec![
        Rung::Identity,
        Rung::Path,
        Rung::Git,
        Rung::Agent,
        Rung::LastEvent,
        Rung::Actions,
    ];
    assert_eq!(
        c.plan_project("alpha-project", 39, 15),
        without_recent,
        "one column short of Recent's 40"
    );
    assert_eq!(
        c.plan_project("alpha-project", 40, 14),
        without_recent,
        "one row short of Recent's 15"
    );
}

#[test]
fn path_outlives_recent_but_not_ten_rows() {
    let c = Case::seeded("normal.json");
    assert_eq!(
        c.plan_project("alpha-project", 44, 10),
        vec![
            Rung::Identity,
            Rung::Path,
            Rung::Git,
            Rung::Agent,
            Rung::LastEvent,
            Rung::Actions,
        ],
        "PROPOSAL §10's 48x14 --mini row: R0-R5, no RECENT"
    );
    assert_eq!(
        c.plan_project("alpha-project", 44, 9),
        vec![
            Rung::Identity,
            Rung::Git,
            Rung::Agent,
            Rung::LastEvent,
            Rung::Actions,
        ],
        "PROPOSAL §9's removal test: path is the first thing cut, above rungs that sit \
         below it on screen — so the result is a subsequence of the ladder, not a prefix"
    );
}

#[test]
fn last_event_needs_thirty_columns() {
    let c = Case::seeded("normal.json");
    assert_eq!(
        c.plan_project("alpha-project", 30, 7),
        vec![
            Rung::Identity,
            Rung::Git,
            Rung::Agent,
            Rung::LastEvent,
            Rung::Actions,
        ],
        "PROPOSAL §3.3's 36x10 mockup: identity, git, agent, last, bare keys"
    );
    assert_eq!(
        c.plan_project("alpha-project", 29, 7),
        vec![Rung::Identity, Rung::Git, Rung::Agent, Rung::Actions],
    );
}

#[test]
fn the_floor_still_renders_four_rungs() {
    let c = Case::seeded("normal.json");
    assert_eq!(
        c.plan_project("alpha-project", 24, 6),
        vec![Rung::Identity, Rung::Git, Rung::Agent, Rung::Actions],
        "PROPOSAL §3.4's 24x6 floor: identity, the git facts, the sparkline, and the keys \
         that still work"
    );
}

#[test]
fn below_the_floor_plans_nothing() {
    let c = Case::seeded("normal.json");
    for (w, h) in [(23, 40), (200, 5), (23, 5), (0, 0), (1, 1)] {
        assert!(
            c.plan_project("alpha-project", w, h).is_empty(),
            "{w}x{h} is below the {}x{} floor and must plan no rungs at all — an empty \
             plan is the mount's signal to render the too-small message",
            petri::focus::MIN_FOCUS_WIDTH,
            petri::focus::MIN_FOCUS_HEIGHT,
        );
    }
}

// ---------------------------------------------------------------------------
// Recent's feed gate. Two distinct causes of absence, asserted at the SAME rect
// so an implementation cannot satisfy one by getting the other wrong.
// ---------------------------------------------------------------------------

/// A rect that clears every geometric gate `Recent` has, so anything below is about the
/// feed and nothing else.
const RECENT_FITS: (u16, u16) = (76, 20);

#[test]
fn recent_is_present_when_the_feed_has_seen_this_project() {
    let c = Case::seeded("normal.json");
    let (w, h) = RECENT_FITS;
    assert!(
        c.plan_project("alpha-project", w, h)
            .contains(&Rung::Recent),
        "control case: at {w}x{h} with a seeded feed, Recent is in the plan"
    );
}

#[test]
fn recent_is_absent_when_there_is_no_feed_at_all() {
    let c = Case::without_feed("normal.json");
    let (w, h) = RECENT_FITS;
    assert!(
        !c.plan_project("alpha-project", w, h)
            .contains(&Rung::Recent),
        "`feed: None` is a real state — `--mini` may start before two snapshots have been \
         diffed. The rung is absent, never an empty box"
    );
}

#[test]
fn recent_is_absent_when_the_feed_is_empty() {
    let c = Case::with_empty_feed("normal.json");
    let (w, h) = RECENT_FITS;
    assert!(
        !c.plan_project("alpha-project", w, h)
            .contains(&Rung::Recent),
        "a feed that exists but holds nothing is as empty as no feed"
    );
}

#[test]
fn recent_is_absent_when_the_feed_has_never_seen_this_project() {
    // `iron-depot` has an `agent.last_event_at`, so `FeedState::seeded` gives it a row;
    // `日本語プロジェクト-workspace` has none, so the seeded feed knows nothing about it.
    let c = Case::seeded("hostile.json");
    let (w, h) = RECENT_FITS;
    assert!(
        c.plan_project("iron-depot", w, h).contains(&Rung::Recent),
        "control: the feed does hold a row for iron-depot"
    );
    assert!(
        !c.plan_project("日本語プロジェクト-workspace", w, h)
            .contains(&Rung::Recent),
        "the feed is populated, but not for THIS project — the gate is per-project, not \
         'is the feed non-empty'"
    );
}

#[test]
fn a_section_or_empty_target_plans_the_ladder_without_recent() {
    let c = Case::seeded("normal.json");
    let (w, h) = RECENT_FITS;
    let expected = vec![
        Rung::Identity,
        Rung::Path,
        Rung::Git,
        Rung::Agent,
        Rung::LastEvent,
        Rung::Actions,
    ];
    assert_eq!(
        c.plan(FocusTarget::Section(StatusBucket::Active, 4), w, h),
        expected,
        "a non-project target has no project to filter the feed by, so Recent cannot be \
         planned — but the plan is NOT empty: empty means 'too small' and nothing else, \
         and the empty state is `focus_lines`' job to render"
    );
    assert_eq!(c.plan(FocusTarget::Nothing, w, h), expected);
}

// ---------------------------------------------------------------------------
// Invariants that must hold everywhere, across every fixture and every size —
// the net under the size-by-size cases above.
// ---------------------------------------------------------------------------

fn is_subsequence_of_ladder(rungs: &[Rung]) -> bool {
    let mut ladder = LADDER.iter();
    rungs.iter().all(|r| ladder.any(|l| l == r))
}

#[test]
fn every_plan_is_an_ordered_duplicate_free_subsequence_that_fits() {
    for fixture in ["normal.json", "hostile.json", "minimal.json", "loaded.json"] {
        let c = Case::seeded(fixture);
        for idx in 0..c.radar.projects.len() {
            for w in [0u16, 12, 23, 24, 29, 30, 39, 40, 55, 56, 80, 120, 240] {
                for h in [0u16, 3, 5, 6, 9, 10, 14, 15, 16, 27, 28, 31, 32, 60] {
                    let rungs = c.plan(FocusTarget::Project(idx), w, h);
                    let where_ = format!("{fixture} project {idx} at {w}x{h}");

                    assert!(
                        is_subsequence_of_ladder(&rungs),
                        "{where_}: plan must be in render order with no duplicates, got \
                         {rungs:?}"
                    );

                    let planned_rows: u16 = rungs.iter().map(|r| rung_rows(*r, h)).sum::<u16>();
                    assert!(
                        planned_rows <= h,
                        "{where_}: planned rungs need {planned_rows} rows but the panel has \
                         {h} — the height gates exist precisely so no separate budget check \
                         is needed, which only holds if they are right"
                    );

                    if w >= petri::focus::MIN_FOCUS_WIDTH && h >= petri::focus::MIN_FOCUS_HEIGHT {
                        assert!(
                            rungs.starts_with(&[Rung::Identity]),
                            "{where_}: above the floor, Identity is unconditional — the \
                             panel always says which project it is pointed at"
                        );
                        for required in [Rung::Git, Rung::Agent] {
                            assert!(
                                rungs.contains(&required),
                                "{where_}: {required:?} is unconditional above the floor"
                            );
                        }
                    } else {
                        assert!(rungs.is_empty(), "{where_}: below the floor plans nothing");
                    }
                }
            }
        }
    }
}

#[test]
fn a_plan_never_shrinks_when_the_panel_grows() {
    // Monotonicity: giving the panel more room can only ever add rungs. A gate written as
    // `height == 16` rather than `height >= 16`, or a row budget that double-counts, breaks
    // this and produces the "it was there a moment ago" flicker as a terminal is resized.
    let c = Case::seeded("normal.json");
    let idx = idx_of(&c.radar, "alpha-project");
    for w in [24u16, 40, 56, 80, 120] {
        for h in 6u16..40 {
            let small = c.plan(FocusTarget::Project(idx), w, h);
            let taller = c.plan(FocusTarget::Project(idx), w, h + 1);
            let wider = c.plan(FocusTarget::Project(idx), w + 1, h);
            for grown in [(&taller, "one row taller"), (&wider, "one column wider")] {
                for rung in &small {
                    assert!(
                        grown.0.contains(rung),
                        "{w}x{h} planned {rung:?} but {} does not",
                        grown.1
                    );
                }
            }
        }
    }
}
