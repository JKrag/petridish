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
use petridish_core::schema::{Radar, StatusBucket};
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::feed::FeedState;
use crate::prefs::Prefs;

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
/// | `Recent` | 3 | 40 | 15 | **and** `ctx.feed` carries ≥1 event for this project |
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
    unimplemented!("Phase B / T1a — petri/PLAN-focus-panel.md section 5")
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
    unimplemented!("Phase B — petri/PLAN-focus-panel.md section 5")
}
