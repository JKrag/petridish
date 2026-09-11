//! `TQ-a`/`TQ-b` (`PLAN-focus-panel.md` §8, issue #29 MVP) — quota in the header, on both
//! screens. Protected: authored before the implementation, per the plan's §1 inversion.
//!
//! Two things this file pins and nothing else does:
//!
//!  1. **The elision ladder itself.** `header_lines` has never had one — `split_line` pads
//!     to at least one space and lets a too-long right group overflow, which `Paragraph`
//!     then clips at the frame edge. So every narrow-terminal case below is new behaviour,
//!     not a regression guard, and it is asserted against the *pure* ladder function rather
//!     than a rendered grid so a failure names the rung that misfired.
//!  2. **`None` is omission, never `0%`** (`PROPOSAL-focus-panel.md` §6). A rendered `0%`
//!     reads as "you have used nothing", which is the opposite of "we do not know".
//!
//! Synthetic `Radar`s throughout: every fixture has `quota: null` and `fixtures/` is
//! protected (plan §3), so the *present*-quota cases have nowhere else to come from.
//! `hostile.json` stays the absent-quota case and is used as such below.

use petri::dashboard;
use petridish_core::schema::{
    AgentState, GitState, Project, QuotaState, Radar, SCHEMA_VERSION, StatusBucket,
};
use std::path::PathBuf;

fn load(name: &str) -> Radar {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join(name);
    let text = std::fs::read_to_string(&path).expect("fixture readable");
    serde_json::from_str(&text).expect("fixture deserializes into Radar")
}

fn now() -> chrono::DateTime<chrono::Utc> {
    "2026-09-09T14:22:00Z".parse().expect("pinned now")
}

fn quota(five: Option<u8>, seven: Option<u8>) -> QuotaState {
    QuotaState {
        measured_at: Some(now()),
        five_hour_used_pct: five,
        five_hour_resets_at: None,
        seven_day_used_pct: seven,
        seven_day_resets_at: None,
        // Owned by whichever session wrote `last-status.json` last, attributable to no
        // project (`DATA-5`). Set deliberately, so the "never rendered" assertion below
        // is testing a rule rather than an absence.
        context_used_pct: Some(73),
    }
}

fn project(id: &str) -> Project {
    Project {
        id: id.to_string(),
        name: id.to_string(),
        path: format!("/repos/{id}"),
        category: "default".to_string(),
        parent_path: None,
        is_foreign: false,
        git: GitState::not_a_repo(),
        agent: AgentState::idle_unknown(),
        last_activity_at: None,
        status_bucket: StatusBucket::Active,
        agent_activity: Vec::new(),
    }
}

/// A radar with `n` projects and the given quota.
///
/// `updated_at` is the **live** clock, not `now()`'s pinned literal. `plan_layout` compares
/// it against `Utc::now()` to decide whether to spend a row on the staleness banner, so a
/// pinned stamp would quietly add a row to the frame the day after this was written and push
/// the header off row 0 — `PLAN-focus-panel.md` §10's time-bomb shape, which has already
/// cost this repo a day once (`swab/src/sensors/quota.rs`, fixed in `90aed29`). The pinned
/// `now()` stays where it belongs: injected into the functions that take a clock.
fn radar(n: usize, q: Option<QuotaState>) -> Radar {
    Radar {
        schema_version: SCHEMA_VERSION,
        updated_at: chrono::Utc::now(),
        scan_duration_ms: 300,
        projects: (0..n).map(|i| project(&format!("p{i}"))).collect(),
        quota: q,
    }
}

// ---------------------------------------------------------------- the segment itself

#[test]
fn both_percentages_render_with_the_headers_own_separator_vocabulary() {
    assert_eq!(
        dashboard::quota_segment(Some(&quota(Some(16), Some(1))), false).as_deref(),
        Some("5h 16% · 7d 1%"),
        "PROPOSAL §6 pins `5h 16% · 7d 1%` — spaces and `·`, matching the separators the \
         header already uses, not `5h:16% 7d:1%`"
    );
}

#[test]
fn the_compressed_form_drops_the_labels_not_the_numbers() {
    assert_eq!(
        dashboard::quota_segment(Some(&quota(Some(16), Some(1))), true).as_deref(),
        Some("16%/1%")
    );
}

#[test]
fn absent_quota_is_no_segment_at_all() {
    assert_eq!(dashboard::quota_segment(None, false), None);
    assert_eq!(dashboard::quota_segment(None, true), None);
}

#[test]
fn a_quota_state_whose_every_percentage_is_none_is_also_no_segment() {
    // Field-by-field degradation is real: `swab`'s sensor populates what it can parse.
    assert_eq!(
        dashboard::quota_segment(Some(&quota(None, None)), false),
        None
    );
    assert_eq!(
        dashboard::quota_segment(Some(&quota(None, None)), true),
        None
    );
}

#[test]
fn a_missing_half_is_omitted_never_rendered_as_zero() {
    let only_five = quota(Some(16), None);
    assert_eq!(
        dashboard::quota_segment(Some(&only_five), false).as_deref(),
        Some("5h 16%"),
        "an absent 7d must vanish; `7d 0%` would claim we measured zero usage"
    );
    let only_seven = quota(None, Some(1));
    assert_eq!(
        dashboard::quota_segment(Some(&only_seven), false).as_deref(),
        Some("7d 1%")
    );
    for text in [
        dashboard::quota_segment(Some(&only_five), true),
        dashboard::quota_segment(Some(&only_seven), true),
    ]
    .into_iter()
    .flatten()
    {
        assert!(!text.contains('0'), "no fabricated zero, got {text:?}");
    }
}

#[test]
fn a_lone_half_keeps_its_label_even_compressed() {
    // `16%/1%` is only unambiguous because both halves are present in a fixed order. One
    // number on its own must say which window it belongs to, and at six columns the
    // labelled form is no wider than the compressed pair anyway.
    assert_eq!(
        dashboard::quota_segment(Some(&quota(Some(16), None)), true).as_deref(),
        Some("5h 16%")
    );
    assert_eq!(
        dashboard::quota_segment(Some(&quota(None, Some(1))), true).as_deref(),
        Some("7d 1%")
    );
}

#[test]
fn context_used_pct_is_never_rendered() {
    // `DATA-5`: it belongs to the most recent session on this machine, not to the fleet and
    // not to any project. 73 is `quota()`'s value; `ctx` is the label it would carry.
    let group =
        dashboard::header_right_group(&radar(14, Some(quota(Some(16), Some(1)))), &now(), 0.3, 200);
    assert!(
        !group.contains("73") && !group.contains("ctx"),
        "context_used_pct must not reach the header, got {group:?}"
    );
}

// ---------------------------------------------------------------- the elision ladder

/// The full group at a comfortable width — `PROPOSAL §6`'s mockup line, verbatim in content.
#[test]
fn the_full_group_carries_projects_quota_clock_and_scan() {
    let group =
        dashboard::header_right_group(&radar(14, Some(quota(Some(16), Some(1)))), &now(), 0.3, 100);
    assert_eq!(group, "14 projects · 5h 16% · 7d 1% · 14:22 · scan 0.3s");
}

/// Every rung of the ladder, narrowest-first, at the exact width that forces it. The widths
/// are derived from the strings rather than pinned by hand: what is being asserted is the
/// *order* things drop in, which is the decision, not the arithmetic.
#[test]
fn the_ladder_drops_scan_then_projects_then_the_clock_then_compresses_quota() {
    let r = radar(14, Some(quota(Some(16), Some(1))));
    let at = |w: usize| dashboard::header_right_group(&r, &now(), 0.3, w);

    let rungs = [
        "14 projects · 5h 16% · 7d 1% · 14:22 · scan 0.3s",
        "14 projects · 5h 16% · 7d 1% · 14:22",
        "5h 16% · 7d 1% · 14:22",
        "5h 16% · 7d 1%",
        "16%/1%",
        "",
    ];

    // Walk widths down from "everything fits" to zero and collect the distinct groups seen,
    // in order. That is exactly the ladder, and it catches a rung that is skipped or one
    // that is emitted out of order — neither of which a per-width assertion would see.
    let mut seen: Vec<String> = Vec::new();
    for w in (0..=100).rev() {
        let g = at(w);
        if seen.last().map(String::as_str) != Some(g.as_str()) {
            seen.push(g);
        }
    }
    assert_eq!(
        seen, rungs,
        "the ladder must degrade in PROPOSAL §6's stated order"
    );
}

#[test]
fn quota_outranks_the_clock() {
    // Stated as a deliberate call in PROPOSAL §6: "a clock is available everywhere, and the
    // burn number is the thing you opened this pane to keep half an eye on."
    let r = radar(14, Some(quota(Some(16), Some(1))));
    let width = " petri · dashboard ".chars().count() + "5h 16% · 7d 1%".chars().count() + 2;
    let group = dashboard::header_right_group(&r, &now(), 0.3, width);
    assert!(group.contains("5h 16%"), "got {group:?}");
    assert!(
        !group.contains("14:22"),
        "the clock must go first, got {group:?}"
    );
}

#[test]
fn the_ladder_without_quota_is_the_pre_existing_group() {
    // The absent-quota path must reduce to exactly what the header rendered before this
    // feature — otherwise every existing header assertion is quietly a new assertion.
    let r = radar(14, None);
    assert_eq!(
        dashboard::header_right_group(&r, &now(), 0.3, 100),
        "14 projects · 14:22 · scan 0.3s"
    );
}

#[test]
fn hostile_json_has_no_quota_and_therefore_no_segment() {
    let r = load("hostile.json");
    assert!(
        r.quota.is_none(),
        "hostile.json is the absent-quota fixture"
    );
    let group = dashboard::header_right_group(&r, &now(), 0.3, 100);
    assert!(
        !group.contains('%'),
        "no percentage may appear, got {group:?}"
    );
}

#[test]
fn the_group_never_overflows_the_width_it_was_given() {
    // The reason the ladder exists: `split_line` does not truncate, so an over-long right
    // group runs off the frame and takes the title with it.
    let r = radar(14, Some(quota(Some(100), Some(100))));
    let left = " petri · dashboard ".chars().count();
    for w in 0..=120 {
        let group = dashboard::header_right_group(&r, &now(), 0.3, w);
        if group.is_empty() {
            continue;
        }
        assert!(
            left + group.chars().count() + 2 <= w,
            "at width {w} the group {group:?} leaves no room for the title, a pad column \
             and the trailing space"
        );
    }
}

// ---------------------------------------------------------------- on screen

#[test]
fn the_rendered_dashboard_header_carries_the_quota_segment() {
    use petri::dashboard::DashboardState;
    use ratatui::{Terminal, backend::TestBackend};

    let radar = radar(14, Some(quota(Some(16), Some(1))));
    let state = DashboardState::new(&radar);
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).expect("test backend");
    terminal
        .draw(|f| dashboard::render(f, &radar, &state, &petri::feed::FeedState::default()))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let row0: String = (0..100)
        .map(|x| buffer.cell((x, 0)).expect("cell").symbol())
        .collect::<Vec<_>>()
        .concat();
    assert!(
        row0.contains("5h 16%"),
        "row 0 must carry the 5h burn, got {row0:?}"
    );
    assert!(
        row0.contains("7d 1%"),
        "row 0 must carry the 7d burn, got {row0:?}"
    );
}

// ---------------------------------------------------------------- TQ-b: the `--mini` header

/// `PROPOSAL §3.2`'s 60×20 mockup: ` petri · project-radar      5h 16% · 7d 1% · ▲ 4m`.
#[test]
fn the_mini_header_carries_quota_and_the_silence_indicator() {
    let row = mini_header_row(60, 20, Some(quota(Some(16), Some(1))));
    assert!(
        row.contains("petri · project-radar"),
        "identity must survive, got {row:?}"
    );
    assert!(row.contains("5h 16%"), "got {row:?}");
    assert!(row.contains("7d 1%"), "got {row:?}");
}

/// `PROPOSAL §3.3`'s 36×10 mockup shows ` petri · project-radar        ▲ 4m` — quota gone,
/// the silence indicator kept. That pins the mini ladder's order the other way round from
/// the Dashboard's: here the per-project fact outranks the fleet-wide one, because a
/// `--mini` pane exists to watch one project.
#[test]
fn the_mini_ladder_drops_quota_before_the_silence_indicator() {
    // The mockup's own dimensions and its own 13-column project name: what makes 36
    // columns tight is that the identity half is already spending 22 of them.
    let row = mini_header_row(36, 10, Some(quota(Some(16), Some(1))));
    assert!(
        !row.contains('%'),
        "quota must have dropped at 36 columns, got {row:?}"
    );
    assert!(
        row.contains('\u{25B2}') || row.contains("silent"),
        "the silence indicator must survive quota, got {row:?}"
    );
}

#[test]
fn the_mini_header_omits_quota_entirely_when_there_is_none() {
    let row = mini_header_row(60, 20, None);
    assert!(!row.contains('%'), "got {row:?}");
    assert!(row.contains("petri · project-radar"), "got {row:?}");
}

/// Render one `--mini` frame and hand back row 0.
fn mini_header_row(w: u16, h: u16, q: Option<QuotaState>) -> String {
    use petri::focus::{FocusCtx, FocusTarget, render_mini};
    use petri::prefs::Prefs;
    use ratatui::{Terminal, backend::TestBackend};

    let mut radar = radar(1, q);
    // The mockups' project, name length included — the identity half's width is half of
    // what makes the ladder fire.
    radar.projects[0].name = "project-radar".to_string();
    // A waiting project, so the right group has a `▲ 4m` to carry — the mockups' case.
    radar.projects[0].agent.waiting_since = Some(now() - chrono::Duration::minutes(4));
    radar.projects[0].agent.active_agent = Some("claude-code".to_string());

    let prefs = Prefs::default();
    let ctx = FocusCtx {
        radar: &radar,
        target: FocusTarget::Project(0),
        now: now(),
        feed: None,
        prefs: &prefs,
    };
    let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test backend");
    terminal
        .draw(|f| render_mini(f, f.area(), &ctx))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    (0..w)
        .map(|x| buffer.cell((x, 0)).expect("cell").symbol())
        .collect::<Vec<_>>()
        .concat()
}
