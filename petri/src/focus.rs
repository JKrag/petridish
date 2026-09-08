//! The **focus panel** — one renderer for "what is the state of this one project, and what
//! can I do about it right now?", mounted at three sizes (`petri/PROPOSAL-focus-panel.md`
//! §1): a popup over the Dashboard (#30), the whole screen (`petri --mini`, #31), and the
//! Browser's existing detail popup (#32).
//!
//! **Phase A scaffold.** Every function below is `unimplemented!()`. The tests in
//! `petri/tests/s11_focus_plan.rs` and `petri/tests/s11_focus_render.rs` were written
//! against this signature *before* any implementation exists and are the spec for it
//! (`petri/PLAN-focus-panel.md` §1) — they fail on purpose until Phase B lands.
//!
//! ## The structural decision
//!
//! `plan_rungs` (which rungs fit) is separated from `focus_lines` (what they say), mirroring
//! `dashboard.rs`'s existing `plan_layout`/`DashPlan` idiom. The responsive behaviour is the
//! part most likely to be got wrong and least likely to *look* wrong, so it is unit-testable
//! at a pinned `Rect` with no `TestBackend` involved.
//!
//! ## Borders: ratatui's default set, deliberately
//!
//! The proposal's §3.1 mockup draws the popup with `╭╮╰╯`, which are **not** on the glyph
//! allowlist (`petri/tests/glyph_portability.rs`), and `PLAN-focus-panel.md` §9 makes a new
//! glyph a stop-and-escalate signal rather than a line to add mid-round. Decision, made here
//! so the mount task (T5) does not have to stop for it: **the popup uses ratatui's default
//! `Borders::ALL`** (`┌┐└┘─│`), the same as `picker.rs` and `help.rs` already do. `--mini`
//! draws no border at all (§9: the terminal edge already frames it). That the gate does not
//! see ratatui-generated border characters at all is a real, pre-existing hole in it,
//! recorded in `IDEAS.md` §5 — it is not this feature's to fix, and not this feature's to
//! widen either.

// Scaffold-only: every body is `unimplemented!()`, so every parameter is unused. Removed in
// the last task of Phase B, once no body is a stub any more.
#![allow(unused_variables)]

use chrono::{DateTime, Utc};
use petridish_core::present;
use petridish_core::schema::{Project, Radar, StatusBucket};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::feed::FeedState;
use crate::prefs::Prefs;
use crate::theme;

/// Narrowest panel that renders anything at all. Below this the mount shows the
/// terminal-too-small message naming these dimensions (`PROPOSAL-focus-panel.md` §3.5).
pub const MIN_FOCUS_WIDTH: u16 = 24;

/// Shortest panel that renders anything at all. See `MIN_FOCUS_WIDTH`.
pub const MIN_FOCUS_HEIGHT: u16 = 6;

/// What the panel is pointed at.
///
/// Not a bare `usize`: the Dashboard's cursor visits section headers too (`SPEC.md` §3.2)
/// and every collapsed-strip entry is its own stop, so a cursor-following popup lands on a
/// non-project stop on essentially every `j`/`k` walk. The empty selection must therefore be
/// representable and must not panic — the same requirement `SPEC.md` §3.1 already places on
/// the Browser's detail pane, here extended to the Dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusTarget {
    /// Index into `Radar::projects`. Never held across a reload by a caller — the scanner
    /// re-sorts on every scan (`SPEC.md` §4.3).
    Project(usize),
    /// The cursor is on a section header (or a collapsed-strip entry): the bucket and how
    /// many projects it holds. Renders the "nothing focused" state naming the section.
    Section(StatusBucket, usize),
    /// No selection at all.
    Nothing,
}

/// The content ladder (`PROPOSAL-focus-panel.md` §2), in **render** order.
///
/// `plan_rungs` returns a subset of these in this order. Note that the order rungs are
/// *rendered* in is not the order they are *dropped* in — §9's removal test cuts `Path`
/// first, above rungs that sit below it on screen — so the returned vector is a subsequence
/// of this list, not necessarily a prefix of it. See `plan_rungs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rung {
    /// R0 — glyph, name, dirty marker, `✎N`; right: waiting/silence/no-agent.
    Identity,
    /// R1 — the `~`-abbreviated path.
    Path,
    /// R2 — branch · commit age + the 14-day daily-commits sparkline.
    Git,
    /// R3 — agent · session + the activity sparkline.
    Agent,
    /// R4 — `last  {event} · {n files} · {HH:MM}`.
    LastEvent,
    /// R5 — the action registry as live affordances.
    Actions,
    /// R6 — this project's slice of the activity feed.
    Recent,
    /// R7 — `mine_last_commit_at` vs `last_commit_at`, plus the GitHub url.
    Repo,
    /// R8 — the worktree family via `parent_path`.
    Tree,
}

/// Everything a rung might need. Borrowed, never owned — the panel is re-planned and
/// re-rendered from scratch on every frame, so nothing here survives a reload.
pub struct FocusCtx<'a> {
    pub radar: &'a Radar,
    pub target: FocusTarget,
    /// Pinned by the caller rather than read from the clock inside a rung, so a render is
    /// reproducible in a test and every rung on one frame agrees about what "now" is.
    pub now: DateTime<Utc>,
    /// `None` is a real state, not an error: `--mini` may start before two snapshots have
    /// been diffed. The `Recent` rung is then absent, not empty.
    pub feed: Option<&'a FeedState>,
    pub prefs: &'a Prefs,
}

/// Which rungs fit in `area`. Pure: no `Frame`, no `Buffer`, no clock read.
///
/// `area` is the panel's **content** rect — the mount has already subtracted its own chrome
/// (a popup's border, `--mini`'s header rule and footer). This function knows nothing about
/// which mount it is serving.
///
/// # The rules, which are chosen rather than derived
///
/// The thresholds below were fitted to `PROPOSAL-focus-panel.md` §10's pressure table and
/// §3.2–§3.4's mockups. They are a design decision expressed as constants, not arithmetic
/// anyone can re-derive — treat them as spec, and change them deliberately.
///
/// - Below `MIN_FOCUS_WIDTH` × `MIN_FOCUS_HEIGHT`, the result is empty. That is the **only**
///   empty return, so an empty vector always means "too small" and never "nothing is
///   selected" — above the floor a `Section`/`Nothing` target still plans the full geometric
///   ladder, and it is `focus_lines` that branches on the target and renders the empty state
///   instead. (`ctx.target` is read here for exactly one thing: resolving `Recent`'s feed
///   gate below, which has no project to look up when the target is not a `Project`.)
/// - `Identity`, `Git` and `Agent` are unconditional above the floor — three rows.
/// - The rest are admitted in this **priority** order, each if its own gates pass:
///   `LastEvent`, `Actions`, `Path`, `Recent`, `Repo`, `Tree`. `Path` sits below `Actions`
///   deliberately (§9's removal test: the path is the first thing cut, because `--mini` is
///   run *from* the project and the path is the one fact the user already knows).
///
/// | Rung | rows | min width | min height | other |
/// |---|---|---|---|---|
/// | `Identity` | 1 | 24 | 6 | |
/// | `Git` | 1 | 24 | 6 | |
/// | `Agent` | 1 | 24 | 6 | |
/// | `LastEvent` | 1 | 30 | 6 | |
/// | `Actions` | 3 if height ≥ 16 else 1 | 24 | 6 | the 3 buys a blank row + the `ACTIONS` label; the 1 is the bare-keys degradation of §3.3 |
/// | `Path` | 1 | 30 | 10 | |
/// | `Recent` | 3 | 40 | 15 | the 3 is the `RECENT` label + two event rows, so unlike `Actions` this rung has no label-less reduced form; **and** `ctx.feed` must carry ≥1 event for this project |
/// | `Repo` | 1 | 56 | 28 | |
/// | `Tree` | 2 | 56 | 32 | |
///
/// `Recent`'s feed condition is the one non-geometric gate, and it is load-bearing twice
/// over: `feed: None` must render the rung as absent rather than as an empty box (T4), and a
/// project the feed has never seen an event for has nothing to put in it.
///
/// There is deliberately **no separate row-budget check**. The height gates were chosen so
/// that the admitted rungs' minimum rows always fit inside `area.height` with slack (5 rows
/// at the floor, 14 at the top of the ladder), and that slack is what the `Recent` rung
/// grows into at render time — §2's "3–8 rows". A budget subtraction on top would be a
/// second, quietly-disagreeing statement of the same rule.
///
/// The returned vector is in `Rung` declaration order (render order), regardless of the
/// priority order rungs were admitted in.
pub fn plan_rungs(area: Rect, ctx: &FocusCtx) -> Vec<Rung> {
    if area.width < MIN_FOCUS_WIDTH || area.height < MIN_FOCUS_HEIGHT {
        return Vec::new();
    }

    let mut admitted = vec![Rung::Identity, Rung::Git, Rung::Agent];

    // Priority order, which is NOT render order — see this function's doc comment and
    // `Rung`'s. Each rung is admitted on its own gates alone; there is no running budget,
    // because the height gates already encode one.
    for rung in [
        Rung::LastEvent,
        Rung::Actions,
        Rung::Path,
        Rung::Recent,
        Rung::Repo,
        Rung::Tree,
    ] {
        let (min_w, min_h) = rung_floor(rung);
        if area.width < min_w || area.height < min_h {
            continue;
        }
        if rung == Rung::Recent && !feed_has_events_for_target(ctx) {
            continue;
        }
        admitted.push(rung);
    }

    // Back into render order, so the caller can walk the result top to bottom.
    admitted.sort();
    admitted
}

/// The `(min width, min height)` gate for one rung, per `plan_rungs`' table.
fn rung_floor(rung: Rung) -> (u16, u16) {
    match rung {
        Rung::Identity | Rung::Git | Rung::Agent | Rung::Actions => {
            (MIN_FOCUS_WIDTH, MIN_FOCUS_HEIGHT)
        }
        Rung::LastEvent => (30, MIN_FOCUS_HEIGHT),
        Rung::Path => (30, 10),
        Rung::Recent => (40, 15),
        Rung::Repo => (56, 28),
        Rung::Tree => (56, 32),
    }
}

/// Rows a rung occupies once rendered. Only `Recent` is elastic — it takes whatever the
/// others leave, between this minimum and `RECENT_MAX_ROWS`.
fn rung_rows(rung: Rung, height: u16) -> u16 {
    match rung {
        // 3 = a blank separator, the `ACTIONS` label, and one row of entries. Below 16 rows
        // there is no room for the label, and §3.3's degradation drops it: the footer's job
        // at that size is to say the keys still work, not to teach them.
        Rung::Actions if height >= 16 => 3,
        // 3 = the `RECENT` label plus two event rows. Unlike `Actions`, this rung has no
        // label-less reduced form: a stamp column with no heading reads as part of whatever
        // sits above it.
        Rung::Recent => 3,
        Rung::Tree => 2,
        _ => 1,
    }
}

/// Ceiling on the `Recent` rung, per `PROPOSAL-focus-panel.md` §2's "3-8 rows". Past this
/// the panel stops being a focus view and starts being the activity feed, which the
/// Dashboard already has.
const RECENT_MAX_ROWS: u16 = 8;

/// Does the feed hold at least one event for whatever the panel is pointed at?
///
/// The one non-geometric gate in `plan_rungs`. A non-`Project` target has no project to
/// filter by and therefore never passes — see `plan_rungs`' doc comment for why that does
/// not make an empty plan ambiguous.
fn feed_has_events_for_target(ctx: &FocusCtx) -> bool {
    let Some(project) = focused_project(ctx) else {
        return false;
    };
    let Some(feed) = ctx.feed else {
        return false;
    };
    feed.events().iter().any(|e| e.project == project.name)
}

/// The project the panel is pointed at, if it is pointed at one that still exists.
///
/// The index is bounds-checked rather than indexed into: `FocusTarget::Project` carries a
/// position in `radar.projects`, the scanner re-sorts and re-populates that vector on every
/// scan (`SPEC.md` §4.3), and a panel that panics because a reload shortened the list is a
/// worse failure than one that renders the empty state for a frame.
fn focused_project<'a>(ctx: &FocusCtx<'a>) -> Option<&'a Project> {
    match ctx.target {
        FocusTarget::Project(idx) => ctx.radar.projects.get(idx),
        _ => None,
    }
}

/// Render the planned rungs as `area.height`-or-fewer lines.
///
/// Branches on `ctx.target` first:
/// - `FocusTarget::Project` — renders `plan_rungs`' output, in order.
/// - `FocusTarget::Section` — the "nothing focused" state, naming the section and its count.
/// - `FocusTarget::Nothing` — the same empty state without a section name.
///
/// Every rung is its own `fn rung_*_lines`, so a Phase B task can implement one without
/// touching the others.
pub fn focus_lines(area: Rect, ctx: &FocusCtx) -> Vec<Line<'static>> {
    let plan = plan_rungs(area, ctx);
    if plan.is_empty() {
        // Below the floor. The *mount* owns the "needs 24x6" message, because only it knows
        // whether the right wording is `--mini`'s or the popup's (`PROPOSAL` §3.5).
        return Vec::new();
    }

    let w = area.width as usize;
    let Some(project) = focused_project(ctx) else {
        return empty_state_lines(ctx, area);
    };

    // `Recent` is the one elastic rung: it takes the rows the fixed ones leave, between its
    // 3-row minimum and `RECENT_MAX_ROWS`. Everything else costs what `rung_rows` says.
    let fixed_rows: u16 = plan
        .iter()
        .filter(|r| **r != Rung::Recent)
        .map(|r| rung_rows(*r, area.height))
        .sum();
    let recent_rows = area.height.saturating_sub(fixed_rows).min(RECENT_MAX_ROWS);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for rung in &plan {
        match rung {
            Rung::Identity => lines.push(identity_line(project, ctx, w)),
            Rung::Path => lines.push(path_line(project, w)),
            Rung::Git => lines.push(git_line(project, ctx, w)),
            Rung::Agent => lines.push(agent_line(project, ctx, w)),
            Rung::LastEvent => lines.extend(last_event_lines(project, w)),
            Rung::Actions => {
                lines.extend(actions_lines(project, ctx, w, area.height >= 16));
            }
            Rung::Recent => lines.extend(recent_lines(project, ctx, w, recent_rows)),
            Rung::Repo => lines.extend(repo_lines(project, ctx, w)),
            Rung::Tree => lines.extend(tree_lines(project, ctx, w)),
        }
    }

    // Belt and braces. The gates are supposed to make this unreachable (and
    // `s11_focus_plan.rs` asserts as much), but a rung that renders one row more than its
    // table entry claims must clip rather than push content off a `Frame`'s bottom edge.
    lines.truncate(area.height as usize);
    lines
}

/// The `FocusTarget::Section` / `FocusTarget::Nothing` states.
///
/// `PROPOSAL` §3.1 option (b): the Dashboard's cursor visits header stops, so a
/// cursor-following popup lands on one constantly. Rather than change what `j`/`k` mean
/// while the popup is open, the panel says plainly what the cursor is on.
fn empty_state_lines(ctx: &FocusCtx, area: Rect) -> Vec<Line<'static>> {
    let w = area.width as usize;
    let dim = Style::default().fg(theme::DIM);
    let mut lines = Vec::new();

    match &ctx.target {
        FocusTarget::Section(bucket, count) => {
            let label = crate::dashboard::SECTION_LABELS
                .iter()
                .find(|(b, _)| b == bucket)
                .map(|(_, l)| *l)
                .unwrap_or("SECTION");
            let noun = if *count == 1 { "project" } else { "projects" };
            lines.push(Line::from(vec![
                Span::raw(INDENT),
                Span::styled(
                    elide(label, w.saturating_sub(1)),
                    Style::default()
                        .fg(crate::theme::bucket_color(*bucket))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" {count} {noun}"), dim),
            ]));
        }
        _ => lines.push(Line::from(Span::styled(
            format!("{INDENT}{}", elide("nothing focused", w.saturating_sub(1))),
            dim,
        ))),
    }

    if area.height >= 3 {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            format!(
                "{INDENT}{}",
                elide("j/k to a project row", w.saturating_sub(1))
            ),
            Style::default().fg(theme::DIMMER),
        )));
    }
    lines.truncate(area.height as usize);
    lines
}

// ---------------------------------------------------------------------------
// Shared row assembly.
// ---------------------------------------------------------------------------

/// Leading indent on every row. One column, not the roomy card's two: the card sits inside
/// a grid cell that already has a gutter, the panel owns its whole rect.
const INDENT: &str = " ";

/// At and above this width a zone row carries its label column. Below it the labels go
/// (`PROPOSAL` §9: at 24 columns a 7-cell label costs 29% of the line).
const LABEL_MIN_WIDTH: usize = 30;

/// Columns the facts may claim before the sparkline takes the rest. Without a cap a wide
/// panel spends everything on a session id nobody reads past the first eight characters.
const FACTS_CAP: usize = 44;

/// A sparkline narrower than this says nothing worth a column; the row drops it and gives
/// the space to the facts instead.
const MIN_SPARKLINE: usize = 8;

/// Fixed width of the scale tag (`14d`, `46m`) at the end of a sparkline row. Fixed, so the
/// tag's width is known before the sparkline is sized — the two would otherwise define each
/// other.
const TAG_WIDTH: usize = 4;

/// `s` cut to `budget` columns with a `…` marker when it does not fit, and **not** padded
/// when it does. `width::fit_exact` pads to exactly the budget, which is right for a column
/// in a table and wrong immediately before another span.
fn elide(s: &str, budget: usize) -> String {
    if crate::width::width(s) <= budget {
        return s.to_string();
    }
    if budget == 0 {
        return String::new();
    }
    format!("{}\u{2026}", crate::width::take_width(s, budget - 1))
}

/// The tail of `s` that fits `budget` columns, marked with a leading `…`. For paths, where
/// the end is the informative part.
fn elide_start(s: &str, budget: usize) -> String {
    if crate::width::width(s) <= budget {
        return s.to_string();
    }
    if budget == 0 {
        return String::new();
    }
    format!("\u{2026}{}", crate::width::take_width_end(s, budget - 1))
}

/// A sparkline for a zone row: the samples, the most it is worth widening to, and how the
/// scale tag is spelled once the width is known.
struct ZoneSpark<'a> {
    samples: &'a [u32],
    /// Cap in samples. Past this the sparkline pads with the zero bar rather than showing
    /// more history, because there is no more history to show.
    max_width: usize,
    style: Style,
    /// `Some(t)` for a fixed tag (`14d` — the git window is a constant); `None` spells the
    /// tag from the chosen width in minutes (`46m` — one agent sample is one tick).
    fixed_tag: Option<&'static str>,
}

struct ZoneSpec<'a> {
    label: &'static str,
    label_style: Style,
    facts: String,
    facts_style: Style,
    spark: Option<ZoneSpark<'a>>,
}

/// One labelled zone row, fitted to exactly `width` columns.
///
/// This is the focus panel's own row assembler rather than `dashboard::zone_row`, and the
/// difference is the reason it exists: the card's version never truncates, because a roomy
/// card is only ever drawn at a width its content is known to fit. The panel is drawn from
/// 24 columns upward, so every field here is a budget — measured in **columns** via
/// `crate::width`, not characters, or `hostile.json`'s CJK name overruns the rect.
fn zone_line(spec: ZoneSpec, width: usize) -> Line<'static> {
    let show_label = width >= LABEL_MIN_WIDTH;
    let label_field = if show_label {
        crate::width::fit_exact(spec.label, crate::dashboard::ZONE_LABEL_WIDTH)
    } else {
        String::new()
    };
    let fixed = crate::width::width(INDENT) + crate::width::width(&label_field);
    let body = width.saturating_sub(fixed);

    if let Some(spark) = spec.spark {
        let want = crate::width::width(&spec.facts).min(FACTS_CAP);
        let spark_w = body
            .saturating_sub(want + 2 + TAG_WIDTH)
            .min(spark.max_width);
        if spark_w >= MIN_SPARKLINE {
            let facts_budget = body - spark_w - 2 - TAG_WIDTH;
            let tag = match spark.fixed_tag {
                Some(t) => t.to_string(),
                None => format!("{spark_w}m"),
            };
            return Line::from(vec![
                Span::raw(INDENT),
                Span::styled(label_field, spec.label_style),
                Span::styled(
                    crate::width::fit_exact(&spec.facts, facts_budget),
                    spec.facts_style,
                ),
                Span::raw("  "),
                Span::styled(
                    crate::dashboard::sparkline_glyphs(spark.samples, spark_w),
                    spark.style,
                ),
                Span::styled(
                    crate::width::fit_exact(&tag, TAG_WIDTH),
                    Style::default().fg(theme::DIM),
                ),
            ]);
        }
    }

    Line::from(vec![
        Span::raw(INDENT),
        Span::styled(label_field, spec.label_style),
        Span::styled(crate::width::fit_exact(&spec.facts, body), spec.facts_style),
    ])
}

/// `dt` as an age relative to `ctx.now`, e.g. `"18h ago"`.
///
/// Deliberately **not** `dashboard::commit_ago`, which reads `Utc::now()` internally. Every
/// derived string on this panel has to come off `ctx.now` or a pinned-clock test measures
/// the calendar instead of the code — the same class of bug as trusting `waiting_since`
/// without re-deriving the latch. `humanize_secs` (the part with no clock in it) is reused.
fn ago(dt: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = now.signed_duration_since(dt).num_seconds().max(0);
    format!("{} ago", crate::dashboard::humanize_secs(secs as u64))
}

/// Seconds since this project last did anything, per `ctx.now`.
fn silence_secs(p: &Project, now: DateTime<Utc>) -> Option<i64> {
    p.last_activity_at
        .map(|dt| now.signed_duration_since(dt).num_seconds().max(0))
}

// ---------------------------------------------------------------------------
// R0-R3 — identity, path, git, agent.
// ---------------------------------------------------------------------------

/// R0: `● alpha-project ✎2` on the left, the attention state on the right.
///
/// The right group has two spellings and can be dropped entirely, in that order, so the
/// project's name never loses columns to it. `▲ waiting on you 4m` degrades to `▲ 4m`
/// (§3.3's mockup) rather than vanishing: `MECH-5` is the one state the product exists to
/// surface, and it keeps all three of its signals — glyph, colour, words — as long as any
/// right group is drawn at all.
fn identity_line(p: &Project, ctx: &FocusCtx, width: usize) -> Line<'static> {
    let waiting = petridish_core::schema::waiting_latch_live(p.agent.waiting_since, ctx.now);
    let silence = silence_secs(p, ctx.now);
    let tier = if waiting {
        theme::DANGER
    } else {
        crate::dashboard::silence_tier_color(silence.unwrap_or(i64::MAX / 2))
    };
    let glyph = if waiting {
        "\u{25B2}"
    } else if p.agent.state == petridish_core::schema::AgentActivity::Working {
        "\u{25CF}"
    } else {
        "\u{25CB}"
    };

    let (right_full, right_short) = if waiting {
        // Age of the latch itself, not of the last activity: "how long have you been
        // blocking this run" is the question, and four minutes and forty are very
        // different situations (`PROPOSAL` §11.6).
        let age = p
            .agent
            .waiting_since
            .and_then(|since| silence_string(ctx.now, since))
            .unwrap_or_default();
        (
            format!("\u{25B2} waiting on you {age}")
                .trim_end()
                .to_string(),
            format!("\u{25B2} {age}").trim_end().to_string(),
        )
    } else if p.agent.active_agent.is_some() {
        match silence {
            Some(s) => {
                let age = crate::dashboard::humanize_secs(s as u64);
                (format!("silent {age}"), age)
            }
            None => ("silent \u{2014}".to_string(), "\u{2014}".to_string()),
        }
    } else {
        ("no agent".to_string(), String::new())
    };

    let dirty = present::dirty_marker(&p.git).trim_end().to_string();
    let uncommitted = if p.git.uncommitted_files > 0 {
        format!(" \u{270E}{}", p.git.uncommitted_files)
    } else {
        String::new()
    };
    let lead = format!("{INDENT}{glyph} ");
    let trailer = format!("{dirty}{uncommitted}");

    // Enough of the name to be worth showing at all before the right group gets any room.
    const MIN_NAME: usize = 8;
    let overhead = crate::width::width(&lead) + crate::width::width(&trailer);
    let room_for = |right: &str| {
        let cost = if right.is_empty() {
            0
        } else {
            crate::width::width(right) + 1
        };
        width >= overhead + MIN_NAME + cost
    };
    let right = if room_for(&right_full) {
        right_full
    } else if room_for(&right_short) {
        right_short
    } else {
        String::new()
    };

    let right_cost = if right.is_empty() {
        0
    } else {
        crate::width::width(&right) + 1
    };
    let name_budget = width.saturating_sub(overhead + right_cost);
    let name = elide(&p.name, name_budget);
    let used = overhead + crate::width::width(&name) + right_cost;
    let pad = width.saturating_sub(used) + if right.is_empty() { 0 } else { 1 };

    Line::from(vec![
        Span::styled(lead, Style::default().fg(tier)),
        Span::styled(
            name,
            Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
        ),
        Span::styled(trailer, Style::default().fg(theme::DANGER)),
        Span::raw(" ".repeat(pad)),
        Span::styled(
            right,
            Style::default().fg(tier).add_modifier(Modifier::BOLD),
        ),
    ])
}

/// How long the latch has been held, as a bare duration. `None` when the stamp is in the
/// future (clock skew), which reads better as no duration than as `0s`.
fn silence_string(now: DateTime<Utc>, since: DateTime<Utc>) -> Option<String> {
    let secs = now.signed_duration_since(since).num_seconds();
    (secs >= 0).then(|| crate::dashboard::humanize_secs(secs as u64))
}

/// R1: the `~`-abbreviated path, keeping its **tail** when it does not fit — the leading
/// directories are the part the user can infer.
fn path_line(p: &Project, width: usize) -> Line<'static> {
    let shown = crate::dashboard::abbreviate_home(&p.path);
    Line::from(Span::styled(
        format!(
            "{INDENT}{}",
            elide_start(&shown, width.saturating_sub(crate::width::width(INDENT)))
        ),
        Style::default().fg(theme::DIM),
    ))
}

/// R2: branch and commit age, paired with git's own daily-commits sparkline.
fn git_line(p: &Project, ctx: &FocusCtx, width: usize) -> Line<'static> {
    let facts = if !p.git.is_repo {
        "not a git repo".to_string()
    } else {
        let branch = p.git.branch.as_deref().unwrap_or("-");
        let dirty = present::dirty_marker(&p.git).trim_end().to_string();
        let commit = match p.git.last_commit_at {
            Some(dt) => format!("commit {}", ago(dt, ctx.now)),
            None => "no commits".to_string(),
        };
        format!("{branch}{dirty} \u{00B7} {commit}")
    };

    zone_line(
        ZoneSpec {
            label: "git",
            label_style: Style::default()
                .fg(theme::BRANCH)
                .add_modifier(Modifier::BOLD),
            facts,
            facts_style: Style::default().fg(theme::DIM),
            spark: p.git.is_repo.then_some(ZoneSpark {
                samples: &p.git.daily_commits,
                max_width: petridish_core::schema::GIT_ACTIVITY_WINDOW_DAYS,
                style: Style::default().fg(theme::BRANCH),
                fixed_tag: Some("14d"),
            }),
        },
        width,
    )
}

/// R3: the agent and its session, paired with the activity ring.
fn agent_line(p: &Project, ctx: &FocusCtx, width: usize) -> Line<'static> {
    let waiting = petridish_core::schema::waiting_latch_live(p.agent.waiting_since, ctx.now);
    let tier = if waiting {
        theme::DANGER
    } else {
        crate::dashboard::silence_tier_color(silence_secs(p, ctx.now).unwrap_or(i64::MAX / 2))
    };

    let facts = match p.agent.active_agent.as_deref() {
        Some(agent) => match p.agent.session_id.as_deref() {
            // By characters, not bytes: a session id is copied verbatim out of a transcript
            // and the schema does not constrain it to ASCII, so a byte slice can land
            // inside a code point and panic.
            Some(session) => {
                let short: String = session.chars().take(18).collect();
                format!("{agent} \u{00B7} sess {short}")
            }
            None => agent.to_string(),
        },
        None => "idle".to_string(),
    };

    zone_line(
        ZoneSpec {
            label: "agent",
            label_style: Style::default().fg(tier).add_modifier(Modifier::BOLD),
            facts,
            facts_style: Style::default().fg(theme::DIM),
            spark: Some(ZoneSpark {
                samples: &p.agent_activity,
                max_width: petridish_core::schema::AGENT_ACTIVITY_WINDOW,
                style: Style::default().fg(tier),
                fixed_tag: None,
            }),
        },
        width,
    )
}

// ---------------------------------------------------------------------------
// Rungs still to come. Each returns nothing until its own task lands, so a
// planned-but-unbuilt rung costs no rows and misreports nothing.
// ---------------------------------------------------------------------------

/// R4: `last  pre tool use · 2 files · 09:14` — what the agent last *did*.
///
/// Carried in the schema since the beginning and shown nowhere outside the feed, which
/// means only for projects that happened to change between two scans.
///
/// **`agent.last_event` being `None` is legitimate, not a defect.** `swab` derives event
/// names from an allowlist, and an unmodelled record type yields `None` on purpose. The row
/// falls back to `feed::agent_detail`, so `claude-code activity · 2 files` is correct
/// output. Do not "fix" it by widening the allowlist — that lives in `swab`.
fn last_event_lines(p: &Project, width: usize) -> Vec<Line<'static>> {
    let body = match p.agent.last_event.as_deref() {
        Some(raw) => {
            let mut s = crate::feed::humanize_event(raw);
            if p.git.is_repo && p.git.uncommitted_files > 0 {
                let n = p.git.uncommitted_files;
                let unit = if n == 1 { "file" } else { "files" };
                s.push_str(&format!(" \u{00B7} {n} {unit}"));
            }
            s
        }
        None => crate::feed::agent_detail(p),
    };
    let facts = match p.agent.last_event_at {
        Some(at) => format!("{body} \u{00B7} {}", at.format("%H:%M")),
        None => body,
    };

    vec![zone_line(
        ZoneSpec {
            label: "last",
            label_style: Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD),
            facts,
            facts_style: Style::default().fg(theme::DIM),
            spark: None,
        },
        width,
    )]
}

fn actions_lines(
    p: &Project,
    ctx: &FocusCtx,
    width: usize,
    with_label: bool,
) -> Vec<Line<'static>> {
    Vec::new() // T3
}

fn recent_lines(p: &Project, ctx: &FocusCtx, width: usize, rows: u16) -> Vec<Line<'static>> {
    Vec::new() // T4
}

/// R7: the repository facts nothing else in the UI shows — whether the newest commit here
/// is yours, and where the remote is.
///
/// `mine_last_commit_at == last_commit_at` is the common case and renders **one** age, not
/// two identical ones. The two-age form is the whole point of the row when they differ:
/// "someone else pushed here" is otherwise unavailable anywhere in `petri`.
fn repo_lines(p: &Project, ctx: &FocusCtx, width: usize) -> Vec<Line<'static>> {
    if !p.git.is_repo {
        return Vec::new();
    }

    let commits = match (p.git.mine_last_commit_at, p.git.last_commit_at) {
        (Some(mine), Some(newest)) if mine != newest => format!(
            "yours {} \u{00B7} newest {}",
            ago(mine, ctx.now),
            ago(newest, ctx.now)
        ),
        (_, Some(newest)) => format!("commit {}", ago(newest, ctx.now)),
        (Some(mine), None) => format!("yours {}", ago(mine, ctx.now)),
        (None, None) => "no commits".to_string(),
    };
    // The scheme is nine columns saying nothing — every url the sensor produces is https.
    let facts = match p.git.github_url.as_deref() {
        Some(url) => format!("{commits} \u{00B7} {}", strip_scheme(url)),
        None => commits,
    };

    vec![zone_line(
        ZoneSpec {
            label: "repo",
            label_style: Style::default()
                .fg(theme::BRANCH)
                .add_modifier(Modifier::BOLD),
            facts,
            facts_style: Style::default().fg(theme::DIM),
            spark: None,
        },
        width,
    )]
}

fn strip_scheme(url: &str) -> &str {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
}

/// R8: the worktree family, via `parent_path`.
///
/// Two rows: what this project is within its family, then who the family is. Today the
/// Browser can only be made to show this by scanning the list by eye.
///
/// A project with no family still renders — `plan_rungs` admits `Tree` on geometry alone —
/// and says so rather than leaving a labelled row blank.
fn tree_lines(p: &Project, ctx: &FocusCtx, width: usize) -> Vec<Line<'static>> {
    let children: Vec<&str> = ctx
        .radar
        .projects
        .iter()
        .filter(|o| o.parent_path.as_deref() == Some(p.path.as_str()))
        .map(|o| o.name.as_str())
        .collect();
    let siblings: Vec<&str> = match p.parent_path.as_deref() {
        Some(parent) => ctx
            .radar
            .projects
            .iter()
            .filter(|o| o.parent_path.as_deref() == Some(parent) && o.path != p.path)
            .map(|o| o.name.as_str())
            .collect(),
        None => Vec::new(),
    };

    let (summary, family) = match p.parent_path.as_deref() {
        Some(parent) => (
            format!("worktree of {}", present::worktree_parent_name(parent)),
            siblings,
        ),
        None if !children.is_empty() => {
            let unit = if children.len() == 1 {
                "worktree"
            } else {
                "worktrees"
            };
            (format!("{} {unit}", children.len()), children)
        }
        None => ("no worktrees".to_string(), Vec::new()),
    };

    let mut lines = vec![zone_line(
        ZoneSpec {
            label: "tree",
            label_style: Style::default()
                .fg(theme::BRANCH)
                .add_modifier(Modifier::BOLD),
            facts: summary,
            facts_style: Style::default().fg(theme::DIM),
            spark: None,
        },
        width,
    )];

    // Second row: the family itself, indented under the label column so it reads as a
    // continuation rather than as another zone.
    let indent = format!("{INDENT}{}", " ".repeat(crate::dashboard::ZONE_LABEL_WIDTH));
    let names = if family.is_empty() {
        String::new()
    } else {
        family.join(" \u{00B7} ")
    };
    lines.push(Line::from(Span::styled(
        format!(
            "{indent}{}",
            elide(&names, width.saturating_sub(crate::width::width(&indent)))
        ),
        Style::default().fg(theme::DIMMER),
    )));
    lines
}
