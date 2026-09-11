//! `T8` (`PLAN-focus-panel.md` §8, `SPACE-3`/issue #33) — the lush density tier, and the
//! surplus-priority rule it needs against the SPACE-1 feed. Protected: authored before the
//! implementation, per the plan's §1 inversion.
//!
//! **The decision this file pins is an ordering, not a size.** `feed_rows_for` has
//! deliberately no ceiling and claims all surplus; the lush tier wants the same rows.
//! `PROPOSAL-focus-panel.md` §5 states the resolution — RUNNING cards get a *bounded* first
//! claim (+2 rows each), the feed takes the remainder under its existing `events + 2` bound
//! — and calls out that leaving it to "whichever code claims first" is not a design. So the
//! tests below assert that cards win the surplus *and* that the win is bounded: the moment
//! the extra rows would cost a project row, the tier falls back.
//!
//! Everything here goes through `plan_layout`, which is pure — no `TestBackend` — except the
//! two rendering tests at the end, which are structural per `SPEC.md` §8.

use petri::dashboard::{self, DashboardState};
use petridish_core::schema::{
    AgentActivity, AgentState, GitState, Project, Radar, SCHEMA_VERSION, StatusBucket,
};
use ratatui::layout::Rect;

fn project(id: &str, bucket: StatusBucket) -> Project {
    Project {
        id: id.to_string(),
        name: id.to_string(),
        path: format!("/repos/{id}"),
        category: "default".to_string(),
        parent_path: None,
        is_foreign: false,
        git: GitState {
            is_repo: true,
            branch: Some("master".to_string()),
            uncommitted_files: 3,
            untracked_files: 0,
            last_commit_at: Some(chrono::Utc::now() - chrono::Duration::hours(2)),
            mine_last_commit_at: Some(chrono::Utc::now() - chrono::Duration::days(4)),
            github_url: Some("https://github.com/JKrag/petridish".to_string()),
            ..GitState::not_a_repo()
        },
        agent: AgentState {
            active_agent: Some("claude-code".to_string()),
            state: AgentActivity::Working,
            last_event: Some("Bash".to_string()),
            last_event_at: Some(chrono::Utc::now() - chrono::Duration::minutes(4)),
            ..AgentState::idle_unknown()
        },
        last_activity_at: Some(chrono::Utc::now() - chrono::Duration::minutes(4)),
        status_bucket: bucket,
        agent_activity: vec![1, 2, 3, 4, 5],
    }
}

fn radar(running: usize, other: &[(StatusBucket, usize)]) -> Radar {
    let mut projects: Vec<Project> = (0..running)
        .map(|i| project(&format!("run{i}"), StatusBucket::Active))
        .collect();
    for (bucket, n) in other {
        for i in 0..*n {
            projects.push(project(&format!("{bucket:?}{i}"), *bucket));
        }
    }
    Radar {
        schema_version: SCHEMA_VERSION,
        // Recent, so `plan_layout`'s staleness banner never eats the row the sizes below
        // are counted against.
        updated_at: chrono::Utc::now(),
        scan_duration_ms: 300,
        projects,
        quota: None,
    }
}

fn plan(radar: &Radar, w: u16, h: u16, feed_events: usize) -> dashboard::DashPlan {
    let state = DashboardState::new(radar);
    dashboard::plan_layout(Rect::new(0, 0, w, h), radar, state.collapsed, feed_events)
}

fn running(plan: &dashboard::DashPlan) -> &dashboard::SectionPlan {
    plan.sections
        .iter()
        .find(|s| s.bucket == StatusBucket::Active)
        .expect("the RUNNING section must be planned")
}

// ---------------------------------------------------------------- the tier itself

/// Two RUNNING projects in one 60-column column: roomy costs `2 + 2*7 = 16` rows, lush
/// `2 + 2*9 = 20`. A 25-row terminal has 20 fleet rows, so lush fits exactly.
#[test]
fn a_tall_terminal_gets_the_lush_tier() {
    let r = radar(2, &[]);
    let p = plan(&r, 60, 25, 0);
    assert!(!p.compact_tier, "25 rows is well above the compact floor");
    assert!(
        p.lush,
        "the surplus is there and RUNNING has first claim on it"
    );
    assert_eq!(
        running(&p).item_span,
        9,
        "a lush card is the roomy card's 6 box rows + 2 extra content rows + the 1-row gap"
    );
    assert_eq!(running(&p).items_shown, 2, "both projects must still fit");
}

/// The bound. At 22 rows there are 17 fleet rows — enough for both roomy cards (16) but not
/// both lush ones (20). The tier must give way rather than truncate a project off the
/// screen: a card that shows two more fields is worth less than a project you can see at all.
#[test]
fn the_tier_yields_rather_than_costing_a_project_row() {
    let r = radar(2, &[]);
    let p = plan(&r, 60, 22, 0);
    assert!(!p.compact_tier);
    assert!(!p.lush, "lush would have truncated a card off the section");
    assert_eq!(running(&p).item_span, 7, "back to the roomy span");
    assert_eq!(running(&p).items_shown, 2);
    assert!(running(&p).truncated_remaining.is_none());
}

/// The same rule across sections, which is the case a per-section check would miss: RUNNING
/// growing can push a *later* section off the screen entirely.
#[test]
fn the_tier_yields_rather_than_skipping_a_later_section() {
    let r = radar(2, &[(StatusBucket::Stale, 3)]);
    let baseline = plan(&radar(2, &[]), 60, 26, 0);
    assert!(baseline.lush, "the same height is lush with RUNNING alone");

    let p = plan(&r, 60, 26, 0);
    let skipped_or_truncated = p.skipped.iter().any(|(b, _)| *b == StatusBucket::Stale)
        || p.sections.iter().any(|s| s.truncated_remaining.is_some());
    assert!(
        !skipped_or_truncated,
        "STALE must still be reachable: {:?} / {:?}",
        p.skipped,
        p.sections
            .iter()
            .map(|s| (s.bucket, s.items_shown, s.truncated_remaining))
            .collect::<Vec<_>>()
    );
}

/// `SPEC.md` §3.2 is explicit that density is driven by the row budget, and the earlier
/// width-driven assumption was already wrong once. A very wide but short terminal must not
/// go lush; a narrow but tall one must.
#[test]
fn the_tier_is_driven_by_the_row_budget_not_by_width() {
    let r = radar(2, &[]);
    assert!(
        !plan(&r, 200, 21, 0).lush,
        "200 columns cannot buy rows the terminal does not have"
    );
    assert!(
        plan(&r, 60, 25, 0).lush,
        "60 columns is the narrowest roomy card there is, and it still goes lush when tall"
    );
}

#[test]
fn the_compact_tier_is_untouched_by_all_of_this() {
    let r = radar(2, &[]);
    let p = plan(&r, 200, 21, 0);
    assert!(
        p.compact_tier,
        "21 rows is 16 fleet rows, at the compact floor"
    );
    assert!(!p.lush);
    assert_eq!(running(&p).item_span, 1, "compact rows stay one row each");
}

// ---------------------------------------------------------------- against the feed

/// The surplus-priority rule, stated as an ordering: the cards claim first. This is the
/// decision `PROPOSAL` §5 says must be explicit rather than emergent.
#[test]
fn cards_claim_the_surplus_before_the_feed_does() {
    let r = radar(2, &[]);
    let tall = plan(&r, 60, 25, 12);
    assert!(tall.lush);

    // What the feed would have had if the cards had not grown: the same terminal, the same
    // events, four fewer rows spent on cards.
    let roomy_used = 2 + 2 * 7;
    let lush_used = 2 + 2 * 9;
    assert_eq!(
        tall.feed_rows,
        dashboard::feed_rows_for(
            false,
            tall.fleet_rows,
            lush_used,
            &tall.sections,
            &tall.skipped,
            12
        ),
    );
    assert!(
        tall.feed_rows
            < dashboard::feed_rows_for(
                false,
                tall.fleet_rows,
                roomy_used,
                &tall.sections,
                &tall.skipped,
                12
            ),
        "the feed must actually have given rows up — otherwise this test proves nothing"
    );
}

/// The other half of the bound: what the feed gives up is surplus, never a project row.
///
/// Asserted against the plan the same terminal would have produced with the tier off, which
/// is the only way the guarantee is observable — it is comparative by nature. Swept across
/// every height and two widths rather than spot-checked, because the fallback is a
/// whole-plan comparison and the interesting failures sit at one awkward height.
#[test]
fn the_tier_never_costs_a_project_row_at_any_height() {
    let r = radar(3, &[(StatusBucket::InFlight, 4), (StatusBucket::Stale, 5)]);
    let mut saw_lush = false;
    for w in [60u16, 140] {
        for h in 6..=60u16 {
            let state = DashboardState::new(&r);
            let area = Rect::new(0, 0, w, h);
            let with = dashboard::plan_layout_at_density(area, &r, state.collapsed, 8, true);
            let without = dashboard::plan_layout_at_density(area, &r, state.collapsed, 8, false);
            if !with.lush {
                // Nothing to prove: the fallback already took the base plan, and the two
                // must then be the same layout.
                assert_eq!(
                    shown(&with),
                    shown(&without),
                    "at {w}x{h} the tier is off, so the plan must be the base plan"
                );
                continue;
            }
            saw_lush = true;
            assert!(
                shown(&with) >= shown(&without),
                "at {w}x{h} the lush tier hid a project: {} vs {}",
                shown(&with),
                shown(&without)
            );
            assert_eq!(
                with.skipped.len(),
                without.skipped.len(),
                "at {w}x{h} the lush tier skipped a section the base plan showed"
            );
            assert!(
                with.sections
                    .iter()
                    .all(|s| s.truncated_remaining.is_none()),
                "at {w}x{h} the lush tier truncated a section, which the fallback forbids"
            );
            assert!(
                with.feed_rows <= without.feed_rows,
                "the surplus has to come from somewhere, and the feed is where"
            );
        }
    }
    assert!(saw_lush, "the sweep must actually reach the lush tier");
}

fn shown(plan: &dashboard::DashPlan) -> usize {
    plan.sections.iter().map(|s| s.items_shown).sum()
}

// ---------------------------------------------------------------- what a lush card shows

fn rendered(radar: &Radar, w: u16, h: u16) -> Vec<String> {
    use ratatui::{Terminal, backend::TestBackend};
    let state = DashboardState::new(radar);
    let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test backend");
    terminal
        .draw(|f| dashboard::render(f, radar, &state, &petri::feed::FeedState::default()))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buffer[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect::<String>()
        })
        .collect()
}

/// Lush = roomy + R4 (last event) + R7 (repo). `IDEAS.md`'s `SURF-8` entry is explicit that
/// #33 shares the focus panel's *content ordering and per-field renderers*, not its layout
/// function — so what is asserted is the two facts arriving, in the card's own zone-row
/// shape.
#[test]
fn a_lush_card_carries_the_last_event_and_repo_rows() {
    let r = radar(2, &[]);
    assert!(plan(&r, 80, 25, 0).lush, "precondition for this test");
    let screen = rendered(&r, 80, 25).join("\n");
    assert!(
        screen.contains("last"),
        "the R4 zone row must appear on a lush card, got:\n{screen}"
    );
    assert!(
        screen.to_lowercase().contains("bash"),
        "R4's content is the agent's last event — humanized by `feed::humanize_event`, which \
         is why the case is not asserted, got:\n{screen}"
    );
    assert!(
        screen.contains("repo"),
        "the R7 zone row must appear on a lush card, got:\n{screen}"
    );
    assert!(
        screen.contains("github.com/JKrag/petridish"),
        "R7 carries the remote, which nothing else on the Dashboard shows, got:\n{screen}"
    );
}

#[test]
fn a_roomy_card_still_carries_neither() {
    let r = radar(2, &[]);
    assert!(!plan(&r, 80, 22, 0).lush, "precondition for this test");
    let screen = rendered(&r, 80, 22).join("\n");
    assert!(
        !screen.contains("github.com/JKrag/petridish"),
        "the roomy card is unchanged by #33, got:\n{screen}"
    );
}

/// `mine_last_commit_at != last_commit_at` is the whole reason R7 exists — "someone else
/// pushed here" is unavailable anywhere else in `petri`. The common case where they are
/// equal must render one age, not two identical ones.
#[test]
fn the_repo_row_renders_one_age_when_the_newest_commit_is_yours() {
    let mut r = radar(2, &[]);
    let stamp = chrono::Utc::now() - chrono::Duration::hours(2);
    for p in &mut r.projects {
        p.git.mine_last_commit_at = Some(stamp);
        p.git.last_commit_at = Some(stamp);
    }
    let screen = rendered(&r, 80, 25).join("\n");
    assert!(
        !screen.contains("yours"),
        "one age, not `yours 2h · newest 2h`, got:\n{screen}"
    );
}

/// A non-repo project has no R7 facts at all, and the card's height is fixed by the
/// section's `item_span` — so the row goes blank rather than being dropped (which would
/// misalign every card below it in the column) or filled with an invented value. Untested,
/// this branch is the one that reads as a rendering bug rather than an honest absence.
#[test]
fn a_lush_card_for_a_non_repo_leaves_the_repo_row_blank_and_keeps_its_height() {
    let mut r = radar(2, &[]);
    r.projects[0].git = GitState::not_a_repo();
    assert!(plan(&r, 80, 25, 0).lush, "precondition for this test");

    let screen = rendered(&r, 80, 25);
    let whole = screen.join("\n");
    // Both cards still drew their bottom border, i.e. neither lost a row.
    assert_eq!(
        whole.matches('\u{256F}').count(),
        2,
        "both cards must keep the same box height, got:\n{whole}"
    );
    // The non-repo card carries no `repo` label and invents no commit age.
    let first_card_end = screen
        .iter()
        .position(|l| l.contains('\u{256F}'))
        .expect("the first card must have a bottom border");
    let first_card = screen[..first_card_end].join("\n");
    assert!(
        first_card.contains("run0"),
        "sanity: the first card is the non-repo one, got:\n{first_card}"
    );
    assert!(
        !first_card.contains("repo "),
        "no `repo` row for a non-repo, got:\n{first_card}"
    );
    assert!(
        first_card.contains("last "),
        "R4 is unaffected — `last_event_facts` never degrades to nothing, got:\n{first_card}"
    );
}
