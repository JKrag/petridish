//! The Dashboard screen (petri/SPEC.md §3.2) — the ambient "does anything need
//! me" monitor across a fleet of unattended runs.
//!
//! Unlike the Browser (S5, `browser.rs`), the Dashboard's section headers ARE
//! selection stops. This is load-bearing, not cosmetic: `STALE`/`COLD` ship
//! collapsed by default, a collapsed section renders zero rows, and if the
//! cursor only ever visited rows there would be no way to select a collapsed
//! section and therefore no way to reopen it. So `DashboardState`'s cursor
//! walks a heterogeneous sequence of stops (`DashRow`) — never a flat
//! `Vec<usize>` of project indices the way `BrowserState::visible` does (see
//! `browser.rs`'s doc comment on `SECTION_ORDER` for why that type does not
//! transfer here unexamined).
//!
//! Consequences enumerated in petri/SPEC.md §3.2, each with a dedicated test
//! in `petri/tests/s6_dashboard.rs`:
//! - Selection never enters a collapsed section's rows (they aren't rendered,
//!   so they aren't stops).
//! - A section with zero projects is not rendered at all and contributes no
//!   stop. Collapsed != empty — a collapsed section with N>0 projects IS a
//!   stop, just one whose rows are hidden.
//! - `Space` on a header toggles that section. `Space` on a row toggles the
//!   section containing that row AND moves selection to that section's
//!   header, so the cursor is never left pointing at a row that no longer
//!   exists once the section collapses.
//! - `Enter` on a header toggles the same as `Space`. `Enter` on a row jumps
//!   to the Browser (wired in `lib.rs`'s `poll_loop`, not part of this
//!   module's own state — `DashboardState` only reports which project was
//!   selected).

use petridish_core::present;
use petridish_core::schema::{AgentActivity, Project, Radar, StatusBucket};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph},
};

/// Below this many *content* rows (post chrome, see `render`'s
/// `available_content`), RUNNING drops from roomy 4-line cards to the same
/// single-line compact row IN FLIGHT/STALE/COLD already use. This is the
/// "corner iTerm split" case worked out in the dashboard redesign discussion:
/// a real 21-row split is wide enough for a roomy card's fields but too
/// short to show more than a handful of them, so density is driven by row
/// budget, not column width — width is spent widening the one line instead.
const COMPACT_TIER_MAX_CONTENT_ROWS: usize = 16;

/// Fewest rows worth spending on the SPACE-1 activity feed: one rule, one label, and at
/// least two event rows. Below that the block is chrome with nothing in it.
const FEED_MIN_ROWS: usize = 4;

/// Ceiling on how much surplus height the feed claims. SPACE-1 is meant to *fill slack*,
/// not to become the Dashboard's main event — on a 100-row terminal the remaining surplus
/// should still be there for `SPACE-2`/`SPACE-3` (auto-expanding STALE/COLD, a third
/// density tier) to spend later.
const FEED_MAX_ROWS: usize = 12;

/// Minimum roomy-card width before a second grid column is added. Below this a real branch
/// name (`analysis/cross-registry-identity`) and its `git`/`agent` zone rows have no room to
/// stay legible — chosen from the card's actual content budget (indent + `ZONE_LABEL_WIDTH` +
/// a usable facts string + gap + a readable sparkline + tag), not a round number. Screen-fill
/// review (dashboard redesign, wide-terminal follow-up): petri was leaving roughly half the
/// terminal blank at real working widths because every section rendered as a single column
/// regardless of how much width was available — this and `MIN_COMPACT_COL_WIDTH` are the fix.
const MIN_ROOMY_CARD_WIDTH: usize = 60;
/// Same idea for the single-line compact rows (STALE/COLD, and RUNNING once it drops into the
/// compact tier) — narrower because these rows carry no sparkline to protect.
const MIN_COMPACT_COL_WIDTH: usize = 45;
/// IN FLIGHT rows carry their own git-activity sparkline (see `shows_git_sparkline`'s doc
/// comment in `plan_layout`), so they need more width than `MIN_COMPACT_COL_WIDTH` before a
/// second grid column is worth it — that constant was sized for the plain fields alone and
/// isn't a safe base to build on here (an early version of this constant just added the
/// sparkline's own width on top of it, which under-counted the plain fields themselves and
/// silently clipped the sparkline off at the computed minimum — the exact bug this comment
/// exists to prevent recurring). Computed from `compact_row_line`'s real field widths: leading
/// space + name(26) + gap + branch(22) + gap + dirty(4) + gap + a worst-case commit-age string
/// ("999d ago", 8) + gap + the `[gh]` marker (4), then the sparkline (`GIT_ACTIVITY_WINDOW_DAYS`
/// samples) + its 2-space gap + `"14d"` tag, plus one more gap column so the two halves never
/// touch.
const MIN_INFLIGHT_COL_WIDTH: usize =
    (1 + 26 + 1 + 22 + 1 + 4 + 1 + 8 + 1 + 4) + 1 + (petridish_core::schema::GIT_ACTIVITY_WINDOW_DAYS + 2 + 3);
/// Column cap for either grid. Past this, cards get so narrow that branch names and paths
/// truncate more than they inform — screen-fill should come from wider cards claiming the
/// extra width, not an unbounded column count.
const MAX_GRID_COLUMNS: usize = 4;
/// Blank columns of separation between adjacent grid columns.
const COLUMN_GUTTER: usize = 2;

/// Physical rows a roomy card's bordered box occupies: 4 content lines (header, `git` zone,
/// `agent` zone, path) + 1 top + 1 bottom border row. Grid-cell bounding + selection review:
/// the earlier borderless card design left grid cells with no visual edge (nothing to tell the
/// eye where one project's card ends and the next column's begins) and highlighted the
/// selected card by painting background color under individual text spans, which left ragged
/// gaps wherever a span didn't reach the card's full width. A real border fixes both: it's the
/// cell boundary the grid was missing, and its color is the selection signal (`render_section`'s
/// roomy branch) — the canonical "border color = focus" pattern (`references/visual-patterns.md`
/// → *Typography in monospace* / *Borders*), so text inside the card no longer needs its own
/// background fill to show selection.
const ROOMY_CARD_BOX_ROWS: usize = 6;
/// Indent for a roomy card's `git`/`agent`/path rows relative to its header — shorter than the
/// pre-border design's 5 spaces since the border itself now provides the card's left edge.
const ZONE_INDENT: &str = "  ";

// The dashboard's truecolor palette lives in `crate::theme` — shared with
// `browser.rs` so the two screens read as one app. See that module's doc
// comment for the ANSI-16 → truecolor history.
use crate::theme;

/// Fixed section order, same as `browser::SECTION_ORDER`.
pub const SECTION_ORDER: [StatusBucket; 4] = [
    StatusBucket::Active,
    StatusBucket::InFlight,
    StatusBucket::Stale,
    StatusBucket::Cold,
];

/// Display labels. `RUNNING`'s slot degrades to `RECENT` when nothing in that
/// section has an active agent at all (petri/SPEC.md §3.2: "because RUNNING
/// would then overstate it") — `render` computes the degraded label at draw
/// time rather than storing it here, since it depends on live project data,
/// not just the bucket.
pub const SECTION_LABELS: [(StatusBucket, &str); 4] = [
    (StatusBucket::Active, "RUNNING"),
    (StatusBucket::InFlight, "IN FLIGHT"),
    (StatusBucket::Stale, "STALE"),
    (StatusBucket::Cold, "COLD"),
];

/// A single stop in the Dashboard's cursor sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashRow {
    /// Section header — a selection stop, always present (even when collapsed).
    Header(StatusBucket),
    /// A project row (only in expanded sections). Index into `radar.projects`.
    Project(usize),
}

/// Per-section collapse state, indexed by position in `SECTION_ORDER`.
/// Defaults (petri/SPEC.md §3.2): `RUNNING`/`IN FLIGHT` (indices 0, 1)
/// expanded, `STALE`/`COLD` (indices 2, 3) collapsed.
pub type CollapsedState = [bool; 4];

pub struct DashboardState {
    pub collapsed: CollapsedState,
    pub visible: Vec<DashRow>,
    pub selected: Option<usize>,
}

/// Position of a `StatusBucket` in `SECTION_ORDER`. Panics if not found (all
/// valid buckets are in the order by construction).
fn section_index(bucket: &StatusBucket) -> usize {
    SECTION_ORDER.iter().position(|b| b == bucket).expect("bucket not in SECTION_ORDER")
}

/// Ceiling on how far "quietest first" can promote a project up the RUNNING
/// list (ADR-0001's original ordering, unbounded). Real-world use surfaced
/// the failure mode: a project whose agent session has just sat open and
/// unused for days will always be quieter than one you actively prompted a
/// few minutes ago, so unbounded quietest-first let the days-old forgotten
/// tab permanently bury the session someone was actually mid-run on — the
/// opposite of "the stalled run is the one that needs you." Past this
/// ceiling, silence has stopped meaning "might be stalled" and started
/// meaning "probably forgotten," so those projects sort into one group
/// *below* everything still under the ceiling, instead of competing with it
/// on raw duration. They stay in RUNNING (per ADR-0001 — a live agent
/// process in a forgotten tab still counts), just not at the top of it.
const RUNNING_ATTENTION_CEILING_S: i64 = 3 * 60 * 60; // 3 hours

/// Silence in seconds for sort purposes: `None` (never had any activity) is
/// maximally silent, same convention the pre-ceiling sort used.
fn silence_secs_for_sort(p: &Project) -> i64 {
    match p.last_activity_at {
        Some(dt) => chrono::Utc::now().signed_duration_since(dt).num_seconds().max(0),
        None => i64::MAX,
    }
}

impl DashboardState {
    /// Membership for the `RUNNING` (Active) section per ADR-0001: a project
    /// counts as running if its own `status_bucket` is `Active`, OR it has at
    /// least one worktree child (`parent_path == Some(this project's path)`)
    /// whose own `status_bucket` is `Active`. `is_foreign` projects are
    /// always excluded. Display-only — never mutates `status_bucket`.
    ///
    /// Ordered quietest-first *within* `RUNNING_ATTENTION_CEILING_S`, then
    /// everything past that ceiling as one group below it (also
    /// quietest-first internally) — see the constant's doc comment for why
    /// unbounded quietest-first got replaced.
    ///
    /// Associated function (no `self`) so `DashboardState::running_membership(&radar)`
    /// is directly callable — matches `petri/tests/s6_dashboard.rs`'s call sites.
    pub fn running_membership(radar: &Radar) -> Vec<usize> {
        let mut members: Vec<usize> = radar
            .projects
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.is_foreign)
            .filter(|(_, p)| {
                if p.status_bucket == StatusBucket::Active {
                    return true;
                }
                // Does THIS project have a worktree CHILD (a project whose
                // parent_path points back at this project's own path) that is
                // itself Active? (Not the other direction: this project's own
                // parent_path pointing at something active.)
                radar
                    .projects
                    .iter()
                    .any(|c| c.parent_path.as_deref() == Some(p.path.as_str()) && c.status_bucket == StatusBucket::Active)
            })
            .map(|(idx, _)| idx)
            .collect();

        members.sort_by(|&a, &b| {
            let a_secs = silence_secs_for_sort(&radar.projects[a]);
            let b_secs = silence_secs_for_sort(&radar.projects[b]);
            let a_forgotten = a_secs >= RUNNING_ATTENTION_CEILING_S;
            let b_forgotten = b_secs >= RUNNING_ATTENTION_CEILING_S;
            // Group first (fresh group before the forgotten group), then
            // *longer* silence first within a group — silence in seconds
            // grows with age, so this is `b` vs `a`, not `a` vs `b`.
            a_forgotten.cmp(&b_forgotten).then(b_secs.cmp(&a_secs))
        });

        members
    }

    /// Build the initial state from `radar`: default collapse state (`STALE`
    /// and `COLD` collapsed), cursor on the first stop (`RUNNING`'s header,
    /// or `None` if there is nothing to show at all).
    pub fn new(radar: &Radar) -> Self {
        Self::with_collapsed(radar, [false, false, true, true])
    }

    /// Build the initial state from `radar` with a caller-supplied collapse
    /// state (petri/SPEC.md §6: `petri.toml` persists which sections are
    /// collapsed across restarts) instead of the hardcoded defaults.
    pub fn with_collapsed(radar: &Radar, collapsed: CollapsedState) -> Self {
        let mut state = DashboardState {
            collapsed,
            visible: Vec::new(),
            selected: None,
        };
        state.rebuild(radar);
        state
    }

    /// Membership list for a given section (RUNNING via `running_membership`,
    /// the other three via a plain `status_bucket` filter excluding
    /// `is_foreign`).
    fn section_members(radar: &Radar, bucket: StatusBucket) -> Vec<usize> {
        if bucket == StatusBucket::Active {
            Self::running_membership(radar)
        } else {
            radar
                .projects
                .iter()
                .enumerate()
                .filter(|(_, p)| !p.is_foreign && p.status_bucket == bucket)
                .map(|(idx, _)| idx)
                .collect()
        }
    }

    /// Recompute `visible` (and clamp/carry `selected`) from `radar` and the
    /// current `collapsed` state. Called by `new` and after any toggle.
    fn rebuild(&mut self, radar: &Radar) {
        let mut visible: Vec<DashRow> = Vec::new();

        for (si, bucket) in SECTION_ORDER.iter().enumerate() {
            let members = Self::section_members(radar, *bucket);

            // Empty section: no header, no stop at all (even if collapsed).
            if members.is_empty() {
                continue;
            }

            // Always a stop — this is the load-bearing part of the spec.
            visible.push(DashRow::Header(*bucket));

            // Rows only if expanded.
            if !self.collapsed[si] {
                for idx in members {
                    visible.push(DashRow::Project(idx));
                }
            }
        }

        let was_empty = visible.is_empty();
        self.visible = visible;
        self.selected = if was_empty { None } else { Some(0) };
    }

    /// Move the cursor by `delta` stops in `visible`, clamped at both ends,
    /// never wrapping. No-op on an empty `visible`.
    pub fn move_selection(&mut self, delta: i32) {
        let len = self.visible.len();
        if len == 0 {
            return;
        }
        let current = self.selected.unwrap_or(0) as i32;
        self.selected = Some((current + delta).clamp(0, len as i32 - 1) as usize);
    }

    /// `Space` (or `Enter` on a header) semantics. If the current stop is a
    /// `Header`, toggle that section's collapse state. If the current stop is
    /// a `Project` row, toggle the collapse state of the section that row
    /// belongs to, then move selection to that section's (now-toggled)
    /// header — the cursor must never be left pointing at a row that just
    /// stopped existing.
    pub fn toggle_selected(&mut self, radar: &Radar) {
        let current = match self.selected {
            Some(i) => i,
            None => return,
        };

        // Walk backward from `current` to the nearest preceding Header — the
        // walk order guarantees that's the containing section, even for a
        // RUNNING-membership row whose own `status_bucket` differs (a cold
        // parent pulled in by an active worktree child).
        let bucket = match self.visible.get(current) {
            Some(DashRow::Header(b)) => *b,
            Some(DashRow::Project(_)) => {
                match self.visible[..=current].iter().rev().find_map(|r| match r {
                    DashRow::Header(b) => Some(*b),
                    DashRow::Project(_) => None,
                }) {
                    Some(b) => b,
                    None => return,
                }
            }
            None => return,
        };

        let si = section_index(&bucket);
        self.collapsed[si] = !self.collapsed[si];
        self.rebuild(radar);

        // Headers are always present (they're the load-bearing stops that
        // survive collapse), so we can safely find this section's header
        // again in the rebuilt visible list.
        if let Some(header_pos) = self.visible.iter().position(|r| *r == DashRow::Header(bucket)) {
            self.selected = Some(header_pos);
        }
    }

    /// The `Project` at the current selection, if the current stop is a row
    /// (not a header, and not out of bounds).
    pub fn selected_project<'a>(&self, radar: &'a Radar) -> Option<&'a Project> {
        match self.selected.and_then(|i| self.visible.get(i)) {
            Some(DashRow::Project(idx)) => radar.projects.get(*idx),
            _ => None,
        }
    }
}

/// Number of grid columns a section gets for a given content width. Shared by roomy RUNNING
/// cards and the single-line compact sections — the only difference between them is
/// `min_col_width` (see `MIN_ROOMY_CARD_WIDTH`/`MIN_COMPACT_COL_WIDTH`'s doc comments).
fn grid_columns(min_col_width: usize, available_width: usize) -> usize {
    if available_width == 0 || min_col_width == 0 {
        return 1;
    }
    let mut columns = 1usize;
    while columns < MAX_GRID_COLUMNS {
        let candidate = columns + 1;
        let usable = available_width.saturating_sub((candidate - 1) * COLUMN_GUTTER);
        if usable / candidate < min_col_width {
            break;
        }
        columns = candidate;
    }
    columns
}

/// The width one column gets once `grid_columns` has decided how many there are — the extra
/// width past `min_col_width` (a wide terminal with only 2 columns' worth of content) is
/// distributed evenly, which is what lets the agent sparkline grow past its old fixed 20-sample
/// width (see `agent_sparkline_width_for`) instead of leaving the extra width blank.
fn column_width(available_width: usize, columns: usize) -> usize {
    if columns == 0 {
        return available_width;
    }
    let usable = available_width.saturating_sub((columns - 1) * COLUMN_GUTTER);
    usable / columns
}

/// One section's resolved geometry within a `DashPlan`. `render` turns this into real `Rect`s;
/// `plan_layout` computes it from nothing but counts, so it's assertable without a `Frame`.
pub struct SectionPlan {
    pub bucket: StatusBucket,
    pub member_count: usize,
    /// Rows spent on chrome (rule-above [if not the first section] + label + rule-below): 2 or 3.
    pub chrome_rows: usize,
    pub columns: usize,
    pub card_width: usize,
    /// Physical rows one item occupies: 5 for a roomy RUNNING card, 1 for a compact row.
    pub item_span: usize,
    pub items_shown: usize,
    /// Grid rows actually rendered (`items_shown` divided across `columns`, rounded up).
    pub grid_rows: usize,
    /// `Some(remaining)` when this section's own rows were truncated mid-render (its header
    /// fit, but not every row) — distinct from `DashPlan::skipped`, which is sections that had
    /// no room even for their header.
    pub truncated_remaining: Option<usize>,
}

/// A fully resolved Dashboard layout — the pure function `render` builds before touching a
/// `Frame`, so every breakpoint decision (columns per section, card width, which sections fit)
/// is unit-testable at a pinned `Rect` + synthetic `Radar` with no `TestBackend` involved. See
/// `references/ecosystem-rust.md`'s testing section: "extracting layout math into a pure
/// `fn compute_layout(area) -> ...` makes per-size assertions cheap."
///
/// Two things this struct deliberately does NOT carry, both raised in the dashboard redesign
/// discussion as "keep the door open, don't build it now": a quota-gauge rail (`Radar.quota:
/// Option<QuotaState>` already exists in the schema; SPEC.md §7 already names it "the most
/// likely first post-v1 addition") and a >200-col merged Dashboard+Browser pane. Both slot in
/// the same way when someone actually builds them: carve their `Rect` from `fleet` before the
/// per-section column math runs (rail from one edge, secondary pane as its own region), which
/// only touches this function — `render`'s section-drawing loop and `DashboardState`'s cursor
/// are unaffected either way. Not modeled as `Option<Rect>` fields here because nothing reads
/// them yet; add them when the first real consumer exists.
pub struct DashPlan {
    pub compact_tier: bool,
    pub fleet_rows: usize,
    pub sections: Vec<SectionPlan>,
    /// Sections that didn't fit even their own header+count — named in the "not shown" summary
    /// row instead of silently disappearing (SPEC.md §3.2's "truncate, do not scroll").
    pub skipped: Vec<(StatusBucket, usize)>,
    /// Rows granted to the SPACE-1 activity feed, `0` when it is not drawn at all. See
    /// `feed_rows_for` for the rule.
    pub feed_rows: usize,
}

/// How many rows the SPACE-1 activity feed gets out of whatever height the sections left
/// unspent — `0` when it must not be drawn at all.
///
/// **The feed always yields to project rows.** It is drawn only when the fleet is fully
/// shown: no section was skipped for want of a header, and no section truncated its own
/// rows. This is not belt-and-braces — `plan_layout` really can leave surplus *while*
/// truncating: a roomy RUNNING section has `item_span == 7`, so a 20-row budget fits two
/// cards (14 rows), reports `truncated_remaining`, and leaves 5 rows over. Spending those
/// on a feed while projects are hidden behind a `… +N more` marker would invert SPEC.md
/// §3.2's whole priority.
///
/// It is also suppressed in the compact tier: below `COMPACT_TIER_MAX_CONTENT_ROWS` the
/// screen is already rationing space, which is the opposite of the surplus this fills.
///
/// Otherwise: every unspent row, clamped to `FEED_MAX_ROWS`, and `0` rather than a stub
/// block if fewer than `FEED_MIN_ROWS` remain.
pub fn feed_rows_for(
    compact_tier: bool,
    fleet_rows: usize,
    used: usize,
    sections: &[SectionPlan],
    skipped: &[(StatusBucket, usize)],
) -> usize {
    // Rationing height already: the feed is for surplus, and there is none by definition.
    if compact_tier {
        return 0;
    }
    // Anything hidden outranks the feed — a section that lost its header entirely, or one
    // that stopped short with a `… +N more` marker. The second case is the one that needs
    // stating: truncation and surplus genuinely co-occur.
    if !skipped.is_empty() || sections.iter().any(|s| s.truncated_remaining.is_some()) {
        return 0;
    }
    let surplus = fleet_rows.saturating_sub(used);
    if surplus < FEED_MIN_ROWS {
        0
    } else {
        surplus.min(FEED_MAX_ROWS)
    }
}

/// Compute `DashPlan` from `area`, `radar`'s section membership, and which sections
/// `collapsed` hides — pure, no `Frame`/`TestBackend` needed. Mirrors the pre-grid `render`'s
/// budget accounting exactly for the `columns == 1` case (same `fleet_rows` formula, same
/// per-section chrome/truncation rules) — the grid only changes how a section's *own* row
/// budget gets spent, not the overall section-fits-or-not accounting.
pub fn plan_layout(area: Rect, radar: &Radar, collapsed: CollapsedState) -> DashPlan {
    let width = area.width as usize;
    let elapsed_secs = chrono::Utc::now().signed_duration_since(radar.updated_at).num_seconds().max(0);
    let is_stale = elapsed_secs > 86400;

    // 2 header rows (title + heavy rule) + 2 footer rows (light rule + keymap) + 1 reserved row
    // for the cross-section "not shown" summary — see the pre-grid version's comment (now
    // folded in here) for why that reservation is unconditional rather than added only when
    // needed: a real 80-project fleet in a 16-row corner split showed STALE/COLD can fail to
    // fit even their own header, and without an always-reserved row they vanished with zero
    // indication, which is exactly the silent-truncation failure mode SPEC.md §3.2 rules out.
    let fixed_rows = 2 + 2 + usize::from(is_stale);
    let fleet_rows = (area.height as usize).saturating_sub(fixed_rows).saturating_sub(1);
    let compact_tier = fleet_rows <= COMPACT_TIER_MAX_CONTENT_ROWS;

    let mut sections: Vec<SectionPlan> = Vec::new();
    let mut skipped: Vec<(StatusBucket, usize)> = Vec::new();
    let mut used = 0usize;

    'sections: for (si, bucket) in SECTION_ORDER.iter().enumerate() {
        let members = DashboardState::section_members(radar, *bucket);
        if members.is_empty() {
            continue;
        }

        let is_first_section = sections.is_empty();
        let chrome_rows = if is_first_section { 2 } else { 3 };
        if used + chrome_rows > fleet_rows {
            skipped.push((*bucket, members.len()));
            for later_bucket in SECTION_ORDER.iter().skip(si + 1) {
                let later_members = DashboardState::section_members(radar, *later_bucket);
                if !later_members.is_empty() {
                    skipped.push((*later_bucket, later_members.len()));
                }
            }
            break 'sections;
        }
        used += chrome_rows;

        let roomy = *bucket == StatusBucket::Active && !compact_tier;
        // Roomy card physical footprint: 4 content lines + a 2-row border (`ROOMY_CARD_BOX_ROWS`)
        // + a 1-row gap before the next card in the same column. The border itself is now what
        // separates one card from the next (and, via its color, signals selection) — see
        // `render_section`'s roomy branch.
        let item_span = if roomy { ROOMY_CARD_BOX_ROWS + 1 } else { 1 };
        // IN FLIGHT rows carry their own git-activity sparkline (`compact_row_line`'s
        // `show_git_sparkline`) — a deliberate alignment: IN FLIGHT's default upper bound is 14
        // days (`swab`'s `in_flight` threshold), the same span `GIT_ACTIVITY_WINDOW_DAYS`
        // covers, so "how alive is this branch over the window IN FLIGHT itself represents" is
        // exactly what the section is already about. STALE/COLD spans (60 days, unbounded) are
        // not aligned the same way, so they keep the plain compact row.
        let shows_git_sparkline = *bucket == StatusBucket::InFlight;
        let min_col_width = if roomy {
            MIN_ROOMY_CARD_WIDTH
        } else if shows_git_sparkline {
            MIN_INFLIGHT_COL_WIDTH
        } else {
            MIN_COMPACT_COL_WIDTH
        };
        let max_columns_by_width = grid_columns(min_col_width, width);
        let rows_budget = fleet_rows.saturating_sub(used);
        // Column count: a roomy card's individual content genuinely improves with more width
        // (the agent sparkline scales up to real additional history — see
        // `agent_sparkline_width_for`), so RUNNING always claims what the width-driven ladder
        // allows. A compact row's content does NOT scale with width (fixed fields; even the new
        // IN FLIGHT git sparkline is capped at `GIT_ACTIVITY_WINDOW_DAYS` regardless of column
        // width), so gridding one wider than necessary buys nothing but fragments a section that
        // would otherwise read as one clean list — use the fewest columns (starting at 1) that
        // still avoid truncating this section within its own row budget, and only reach for
        // more when a single column genuinely would not fit.
        let columns = if roomy {
            max_columns_by_width
        } else {
            (1..=max_columns_by_width)
                .find(|&c| members.len().div_ceil(c) * item_span <= rows_budget)
                .unwrap_or(max_columns_by_width)
        };
        let card_width = column_width(width, columns);

        let (items_shown, grid_rows, truncated_remaining) = if collapsed[si] {
            (0, 0, None)
        } else {
            let grid_rows_needed = members.len().div_ceil(columns);
            if grid_rows_needed * item_span > rows_budget {
                let rows_that_fit = rows_budget / item_span;
                let shown = (rows_that_fit * columns).min(members.len());
                (shown, rows_that_fit, Some(members.len() - shown))
            } else {
                (members.len(), grid_rows_needed, None)
            }
        };

        used += grid_rows * item_span;
        if truncated_remaining.is_some() {
            used += 1; // the "… +N more" marker row
        }

        sections.push(SectionPlan {
            bucket: *bucket,
            member_count: members.len(),
            chrome_rows,
            columns,
            card_width,
            item_span,
            items_shown,
            grid_rows,
            truncated_remaining,
        });
    }

    let feed_rows = feed_rows_for(compact_tier, fleet_rows, used, &sections, &skipped);
    DashPlan { compact_tier, fleet_rows, sections, skipped, feed_rows }
}

/// Render the Dashboard into `frame`. Per petri/SPEC.md §3.2, and following petripy's actual
/// chrome (`src/petridish/screens.py`'s `_header`/`_section`):
/// - Header: a badged `petri · dashboard` title, project count/clock/scan duration on the
///   right, then a HEAVY double rule (`═`) the full width.
/// - Each section is bracketed by light rules (`─`): one above (skipped for the very first
///   section) and one below its label line.
/// - **Sections lay their items out in a grid**, not a single always-narrow column: `plan_layout`
///   decides how many columns fit at the current width (`grid_columns`), items are assigned
///   row-major (left column, then right, then the next row down) so `j`/`k` still walks a flat
///   sequence — see `DashboardState`'s module doc comment for why the cursor itself didn't need
///   to change for this. `RUNNING` gets roomy cards (`roomy_card_lines`); `IN FLIGHT`/`STALE`/
///   `COLD`, and `RUNNING` once it drops into the compact tier, get single-line rows.
/// - Collapsed sections still render their header + count, but no rows.
/// - **Overflow: truncate, never scroll.** If a section's own rows exceed its share of the
///   height, it stops with a required `… +N more` marker; sections with no room even for their
///   header are named in one summary row instead of disappearing.
/// - Staleness banner when `radar.updated_at` is older than 24h.
/// - Must not panic on an empty `radar.projects`, nor at 0×0 or 1×1.
///
/// Worktree nesting/rollup (indented children, `name · N worktrees` rollup counts in compact
/// sections) is deliberately NOT attempted here — the acceptance gate (`s6_dashboard.rs`'s
/// module doc comment) documents this as ambiguous against the only fixture that exercises it.
pub fn render(
    frame: &mut ratatui::Frame,
    radar: &Radar,
    state: &DashboardState,
    feed: &crate::feed::FeedState,
) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    // Bail at 1 row or shorter: we need at least a header line.
    if area.height <= 1 {
        return;
    }
    let width = area.width as usize;

    let elapsed_secs = chrono::Utc::now().signed_duration_since(radar.updated_at).num_seconds().max(0);
    let is_stale = elapsed_secs > 86400;

    let now = chrono::Utc::now();
    let scan_secs = radar.scan_duration_ms as f64 / 1000.0;

    let plan = plan_layout(area, radar, state.collapsed);

    let [header_area, banner_area, fleet_area, footer_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(u16::from(is_stale)),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .areas(area);

    frame.render_widget(Paragraph::new(header_lines(radar, &now, scan_secs, width)), header_area);

    if is_stale {
        // `▲`, not `⚠` — the latter (U+26A0, Unicode 4.0) is the exact
        // codepoint that rendered as a blank cell on the macOS 14 CI runner
        // under ncurses/wcwidth (petri/SPEC.md §4.2's founding incident). This
        // banner exists specifically so a stale scan can't fail silently;
        // reusing the one glyph proven to do exactly that here would be
        // ironic at best.
        let banner = Line::from(Span::styled(
            format!(" ▲ Data stale (updated {} ago)", humanize_secs(elapsed_secs as u64)),
            Style::default().fg(Color::Black).bg(theme::DANGER).add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(Paragraph::new(vec![banner]), banner_area);
    }

    frame.render_widget(
        Paragraph::new(vec![rule_line(width, Color::DarkGray), footer_line()]),
        footer_area,
    );

    // Carve `fleet_area` into one Rect per section (by its resolved row count) plus a trailing
    // summary row (only sized >0 when something was skipped) and a final `Fill` catch-all for
    // any genuinely-unused height — legitimate when every section's real content already fits
    // (there is no more data to show, not a layout bug); this is also where a future widget
    // rail or secondary pane would claim space, per `DashPlan`'s doc comment.
    let mut section_constraints: Vec<Constraint> = plan
        .sections
        .iter()
        .map(|s| Constraint::Length((s.chrome_rows + s.grid_rows * s.item_span + usize::from(s.truncated_remaining.is_some())) as u16))
        .collect();
    section_constraints.push(Constraint::Length(u16::from(!plan.skipped.is_empty())));
    // The SPACE-1 feed claims the surplus `feed_rows_for` granted it, ahead of the `Fill`
    // catch-all — which stays, because `feed_rows` is 0 whenever the feed must not draw.
    section_constraints.push(Constraint::Length(plan.feed_rows as u16));
    section_constraints.push(Constraint::Fill(1));
    let section_rects = Layout::vertical(section_constraints).split(fleet_area);

    for (plan_idx, section) in plan.sections.iter().enumerate() {
        render_section(frame, section_rects[plan_idx], radar, state, section, width, plan_idx == 0);
    }

    if !plan.skipped.is_empty() {
        let summary = plan
            .skipped
            .iter()
            .map(|(bucket, count)| {
                let label = SECTION_LABELS.iter().find(|(b, _)| b == bucket).map(|(_, l)| *l).unwrap_or("?");
                format!("{label} +{count}")
            })
            .collect::<Vec<_>>()
            .join("  ·  ");
        let summary_rect = section_rects[plan.sections.len()];
        frame.render_widget(
            Paragraph::new(vec![Line::from(Span::styled(
                format!(" … not shown: {summary} — resize taller"),
                Style::default().fg(theme::AGING).add_modifier(Modifier::BOLD),
            ))]),
            summary_rect,
        );
    }

    if plan.feed_rows > 0 {
        let feed_rect = section_rects[plan.sections.len() + 1];
        frame.render_widget(
            Paragraph::new(crate::feed::feed_block_lines(feed, width, plan.feed_rows)),
            feed_rect,
        );
    }
}

/// Render one section's chrome + grid into `rect`, per `SectionPlan`'s resolved geometry.
fn render_section(
    frame: &mut ratatui::Frame,
    rect: Rect,
    radar: &Radar,
    state: &DashboardState,
    section: &SectionPlan,
    fleet_width: usize,
    is_first_section: bool,
) {
    let members = DashboardState::section_members(radar, section.bucket);

    let [chrome_rect, grid_rect, marker_rect] = Layout::vertical([
        Constraint::Length(section.chrome_rows as u16),
        Constraint::Length((section.grid_rows * section.item_span) as u16),
        Constraint::Length(u16::from(section.truncated_remaining.is_some())),
    ])
    .areas(rect);

    let mut chrome_lines = Vec::with_capacity(3);
    if !is_first_section {
        chrome_lines.push(rule_line(fleet_width, Color::DarkGray));
    }
    let is_selected_header = matches!(
        state.selected.and_then(|i| state.visible.get(i)),
        Some(DashRow::Header(b)) if *b == section.bucket
    );
    chrome_lines.push(section_header_line(radar, section.bucket, section.member_count, is_selected_header, fleet_width));
    chrome_lines.push(rule_line(fleet_width, Color::DarkGray));
    frame.render_widget(Paragraph::new(chrome_lines), chrome_rect);

    if section.items_shown > 0 {
        let columns = section.columns.max(1);
        let roomy = section.item_span == ROOMY_CARD_BOX_ROWS + 1;

        // Row-major assignment (column c gets items c, c+columns, c+2*columns, …) — this is
        // what keeps `j`/`k` walking `DashboardState`'s flat `Vec<DashRow>` in plain reading
        // order (left column, then right, then the next row down) even though the grid renders
        // multiple columns; see this module's doc comment on why the cursor didn't need to
        // become 2D for this. Stacking each column's own items top-down independently (below)
        // still lines up row-for-row across columns because every card/row in a section has the
        // same fixed height, regardless of how many total items landed in that column.
        let mut column_members: Vec<Vec<usize>> = vec![Vec::new(); columns];
        for (i, &proj_idx) in members.iter().take(section.items_shown).enumerate() {
            column_members[i % columns].push(proj_idx);
        }

        let mut column_constraints: Vec<Constraint> = Vec::with_capacity(columns * 2);
        for c in 0..columns {
            column_constraints.push(Constraint::Length(section.card_width as u16));
            if c + 1 < columns {
                column_constraints.push(Constraint::Length(COLUMN_GUTTER as u16));
            }
        }
        let column_rects = Layout::horizontal(column_constraints).split(grid_rect);

        for (c, proj_indices) in column_members.into_iter().enumerate() {
            let col_rect = column_rects[c * 2];
            if roomy {
                let inner_width = section.card_width.saturating_sub(2);
                let mut card_constraints: Vec<Constraint> = Vec::with_capacity(proj_indices.len() * 2);
                for i in 0..proj_indices.len() {
                    card_constraints.push(Constraint::Length(ROOMY_CARD_BOX_ROWS as u16));
                    if i + 1 < proj_indices.len() {
                        card_constraints.push(Constraint::Length(1)); // gap before the next card
                    }
                }
                let card_rects = Layout::vertical(card_constraints).split(col_rect);
                for (i, &proj_idx) in proj_indices.iter().enumerate() {
                    let is_selected = matches!(
                        state.selected.and_then(|i| state.visible.get(i)),
                        Some(DashRow::Project(idx)) if *idx == proj_idx
                    );
                    let border_style = if is_selected {
                        Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme::DIMMER)
                    };
                    let block = Block::bordered().border_type(BorderType::Rounded).border_style(border_style);
                    let lines = roomy_card_lines(radar, proj_idx, inner_width);
                    frame.render_widget(Paragraph::new(lines).block(block), card_rects[i * 2]);
                }
            } else {
                let lines: Vec<Line<'static>> = proj_indices
                    .iter()
                    .map(|&proj_idx| {
                        let is_selected = matches!(
                            state.selected.and_then(|i| state.visible.get(i)),
                            Some(DashRow::Project(idx)) if *idx == proj_idx
                        );
                        if section.bucket == StatusBucket::Active {
                            compact_running_row_line(radar, proj_idx, is_selected, section.card_width)
                        } else {
                            compact_row_line(radar, proj_idx, is_selected, section.card_width, section.bucket == StatusBucket::InFlight)
                        }
                    })
                    .collect();
                frame.render_widget(Paragraph::new(lines), col_rect);
            }
        }

        // A vertical rule between compact-row columns — the grid-cell-bounding fix for the
        // one grid type that isn't already boxed. Roomy cards get their own border on all four
        // sides (above), so a second boundary line here would be a redundant signal for real
        // per the clutter-audit rule against stacking multiple cues for one fact.
        if !roomy && columns > 1 {
            for c in 0..columns - 1 {
                let gutter_rect = column_rects[c * 2 + 1];
                let bar: Vec<Line<'static>> = (0..gutter_rect.height)
                    .map(|_| Line::from(Span::styled(" │", Style::default().fg(theme::DIMMER))))
                    .collect();
                frame.render_widget(Paragraph::new(bar), gutter_rect);
            }
        }
    }

    if let Some(remaining) = section.truncated_remaining {
        frame.render_widget(
            Paragraph::new(vec![Line::from(Span::styled(
                format!(" … +{remaining} more"),
                Style::default().fg(theme::AGING).add_modifier(Modifier::BOLD),
            ))]),
            marker_rect,
        );
    }
}

/// Pad or truncate (with a trailing `…`) to exactly `w` display columns, so a
/// variable-length field (branch names in particular: `master` vs
/// `analysis/cross-registry-identity`) doesn't push every later column in a
/// compact row out of alignment with the rows above and below it. Byte-safe
/// truncation point via `char_indices` — branch names can contain non-ASCII.
fn fixed_width(s: &str, w: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= w {
        format!("{s}{}", " ".repeat(w - char_count))
    } else if w == 0 {
        String::new()
    } else {
        let keep = w - 1;
        let truncated: String = s.chars().take(keep).collect();
        format!("{truncated}…")
    }
}

/// A full-width rule line in the given color — `─` for the light rules that
/// bracket each section, reused at `Color::DarkGray` for the footer's rule
/// too. The header's heavy `═` rule is built inline in `header_lines` since
/// it carries its own bold/bright styling.
fn rule_line(width: usize, color: Color) -> Line<'static> {
    Line::from(Span::styled("─".repeat(width), Style::default().fg(color)))
}

/// Split `left`/`right` across `width` columns, padding between them (at
/// least one space) — mirrors petripy's `_split`: a stable label on the
/// left, the volatile value you're actually scanning on the right.
fn split_line(left: String, right: String, width: usize, left_style: Style, right_style: Style) -> Line<'static> {
    let left_len = left.chars().count();
    let right_len = right.chars().count();
    let pad = width.saturating_sub(left_len + right_len).max(1);
    Line::from(vec![
        Span::styled(left, left_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, right_style),
    ])
}

/// A card's `git` or `agent` zone row: a colored, fixed-width row label (`"git    "` /
/// `"agent  "`, padded to `ZONE_LABEL_WIDTH` so both rows' facts start at the same column),
/// then that zone's own facts, then — right-aligned, same column across every card in the
/// section — that zone's own sparkline plus a scale tag (`"14d"`, `"20m"`). Exists so a
/// card's two sparklines (agent-activity, ~one per minute; git daily-commits, one per day)
/// never sit stacked with no visual anchor to the facts they summarize — the redesign this
/// replaces (a single line with the git sparkline dropped into unused middle space) read as
/// "two similar bars thrown at the wall," per the design review that prompted this file's
/// rewrite. Label color carries the zone identity; the scale tag is the non-color fallback
/// so the two rows stay distinguishable under `NO_COLOR` too.
const ZONE_LABEL_WIDTH: usize = 7;

/// Parameters for `zone_row` — bundled into a struct (rather than nine positional
/// arguments) purely to keep the call sites in `roomy_card_lines` readable and to stay
/// under clippy's too-many-arguments threshold.
struct ZoneRowSpec {
    indent: &'static str,
    label: &'static str,
    label_style: Style,
    facts: String,
    facts_style: Style,
    sparkline: String,
    spark_style: Style,
    tag: String,
}

fn zone_row(spec: ZoneRowSpec, width: usize) -> Line<'static> {
    let label_padded = format!("{:<ZONE_LABEL_WIDTH$}", spec.label);
    let left_len = spec.indent.chars().count() + label_padded.chars().count() + spec.facts.chars().count();
    let right_len = spec.sparkline.chars().count() + 2 + spec.tag.chars().count();
    let pad = width.saturating_sub(left_len + right_len).max(1);

    Line::from(vec![
        Span::raw(spec.indent),
        Span::styled(label_padded, spec.label_style),
        Span::styled(spec.facts, spec.facts_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(spec.sparkline, spec.spark_style),
        Span::raw("  "),
        Span::styled(spec.tag, Style::default().fg(theme::DIM)),
    ])
}

/// Header: a badged title, project count/clock/scan duration on the right,
/// then a heavy double rule spanning the full width. The badge (inverted
/// colors on just the app name) plus the heavy rule is what makes this read
/// as a header at a glance, matching petripy's `_header` — a single plain
/// line of text was the thing that "nearly disappears into the rest."
fn header_lines(radar: &Radar, now: &chrono::DateTime<chrono::Utc>, scan_secs: f64, width: usize) -> Vec<Line<'static>> {
    let right = format!("{} projects · {} · scan {scan_secs:.1}s", radar.projects.len(), now.format("%H:%M"));
    let title = split_line(
        " petri · dashboard ".to_string(),
        format!("{right} "),
        width,
        Style::default().fg(Color::Black).bg(theme::ACCENT).add_modifier(Modifier::BOLD),
        Style::default().fg(theme::FG),
    );
    vec![title, Line::from(Span::styled("═".repeat(width), Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)))]
}

/// Section header line: " RUNNING" left, "25" right, between the two light
/// rules `render` pushes around it. RUNNING degrades to RECENT when no
/// member project has an active agent (`agent.active_agent.is_some()`) —
/// petri/SPEC.md §3.2's "because RUNNING would then overstate it", documented
/// interpretation per `s6_snapshot.rs`'s module doc comment.
fn section_header_line(radar: &Radar, bucket: StatusBucket, count: usize, is_selected: bool, width: usize) -> Line<'static> {
    let label: &str = if bucket == StatusBucket::Active {
        let has_agent = DashboardState::running_membership(radar)
            .iter()
            .any(|&idx| radar.projects[idx].agent.active_agent.is_some());
        if has_agent { "RUNNING" } else { "RECENT" }
    } else {
        SECTION_LABELS
            .iter()
            .find(|(b, _)| *b == bucket)
            .map(|(_, l)| *l)
            .expect("SECTION_LABELS covers all buckets in SECTION_ORDER")
    };
    // Section label previews the state of what's inside it, reusing the same
    // gradient `silence_tier_color` applies per-project: RUNNING reads
    // fresh-green, IN FLIGHT amber, STALE grey, COLD dimmer-grey — one
    // palette (`crate::theme::bucket_color`), not a flat yellow for every
    // section regardless of what it actually holds.
    let label_color = crate::theme::bucket_color(bucket);
    let style = if is_selected {
        Style::default().fg(Color::Black).bg(theme::ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(label_color).add_modifier(Modifier::BOLD)
    };
    split_line(format!(" {label}"), format!("{count} "), width, style, style)
}

/// Truecolor gradient on silence age: fresh (<1m, still likely mid-turn) →
/// aging (<1h, worth a glance) → cold (≥1h, silent long enough to actually
/// worry about). See `crate::theme` for the ANSI-16 → truecolor history. The
/// glyph *allowlist* (petri/SPEC.md §4.2) is a separate concern — provenance
/// is a real macOS `wcwidth` bug, not the planning-doc caution this color
/// rule used to be — and this palette choice doesn't relax or affect it.
fn silence_tier_color(secs: i64) -> Color {
    // Reuses the canonical Working/Recent/Idle thresholds
    // (`AGENT_WORKING_MAX_S` = 90s, `AGENT_RECENT_MAX_S` = 30m) rather than a
    // separate set of cutoffs invented for color alone — one silence
    // vocabulary for the whole app, not two that quietly disagree.
    crate::theme::tier_color(petridish_core::schema::agent_state_for_silence(secs))
}

/// Compact single-line row for a RUNNING project once the Dashboard has
/// dropped into the compact density tier (`COMPACT_TIER_MAX_CONTENT_ROWS`).
/// Same fields a roomy card carries, on one line: glyph, name, dirty marker,
/// branch, silence age — silence age still carries the gradient, since
/// "which run is stalling" is exactly what this row exists to answer at a
/// glance.
fn compact_running_row_line(radar: &Radar, proj_idx: usize, is_selected: bool, card_width: usize) -> Line<'static> {
    let p = &radar.projects[proj_idx];
    let silence_secs = match p.last_activity_at {
        Some(dt) => chrono::Utc::now().signed_duration_since(dt).num_seconds().max(0),
        None => i64::MAX / 2,
    };
    let tier_color = silence_tier_color(silence_secs);
    let glyph = match p.agent.state {
        AgentActivity::Working => "●",
        _ => "○",
    };
    let dirty_marker = present::dirty_marker(&p.git);
    let branch = p.git.branch.as_deref().unwrap_or("-");
    let silence_str = match p.last_activity_at {
        Some(_) => format!("silent {}", humanize_secs(silence_secs as u64)),
        None => "silent -".to_string(),
    };
    let agent = p.agent.active_agent.as_deref().unwrap_or("");

    let name_field = format!("{} ", fixed_width(&format!("{}{dirty_marker}", p.name), 26));
    let branch_field = format!("{} ", fixed_width(branch, 22));
    let silence_field = format!("{} ", fixed_width(&silence_str, 12));

    if is_selected {
        return solid_selected_line(&format!(" {glyph} {name_field}{branch_field}{silence_field}{agent}"), card_width);
    }

    Line::from(vec![
        Span::styled(format!(" {glyph} "), Style::default().fg(tier_color)),
        Span::styled(name_field, Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)),
        Span::styled(branch_field, Style::default().fg(theme::BRANCH)),
        Span::styled(silence_field, Style::default().fg(tier_color)),
        Span::styled(agent.to_string(), Style::default().fg(theme::DIM)),
    ])
}

/// Roomy card for a project in the RUNNING section: a header line (identity — glyph, name,
/// dirty markers, overall silence), a `git` zone row (branch, commit age, git-activity
/// sparkline), an `agent` zone row (agent name, session, agent-activity sparkline), and the
/// path. Each sparkline sits directly beside the facts it summarizes rather than two look-alike
/// bars stacked with nothing tying either to its own data — see `zone_row`'s doc comment for
/// why. `card_width` is the card's *inner* content width (its allocated column width minus the
/// 2 columns its border consumes — see `ROOMY_CARD_BOX_ROWS`'s doc comment) — `render_section`
/// wraps these 4 lines in a `Block` whose border color carries the selection signal, so nothing
/// in here needs to change when a card is selected; that's what fixed the ragged
/// background-only-under-some-spans highlight the border replaced.
fn roomy_card_lines(radar: &Radar, proj_idx: usize, card_width: usize) -> Vec<Line<'static>> {
    let p = &radar.projects[proj_idx];
    let glyph = match p.agent.state {
        AgentActivity::Working => "●",
        _ => "○",
    };
    let dirty_marker = present::dirty_marker(&p.git);
    let uncommitted = if p.git.uncommitted_files > 0 { format!(" ✎{}", p.git.uncommitted_files) } else { String::new() };
    let has_agent = p.agent.active_agent.is_some();
    let silence_secs = match p.last_activity_at {
        Some(dt) => chrono::Utc::now().signed_duration_since(dt).num_seconds().max(0),
        None => 0,
    };
    let header_right = if has_agent {
        format!("silent {}", humanize_secs(silence_secs as u64))
    } else {
        "no agent".to_string()
    };

    let tier_color = silence_tier_color(silence_secs);
    let name_style = Style::default().fg(theme::FG).add_modifier(Modifier::BOLD);
    let silence_style = Style::default().fg(tier_color).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(theme::DIM);
    let zone_label_style = |color: Color| Style::default().fg(color).add_modifier(Modifier::BOLD);

    let header = split_line(
        format!(" {glyph} {}{}{}", p.name, dirty_marker, uncommitted),
        header_right,
        card_width,
        name_style,
        silence_style,
    );

    // git zone: branch + commit age, paired with git's own daily-commits sparkline (one
    // sample/day over GIT_ACTIVITY_WINDOW_DAYS — a real "how alive is this branch" signal,
    // not the agent's per-minute activity).
    let branch = p.git.branch.as_deref().unwrap_or("-");
    let dirty_suffix = if dirty_marker.trim().is_empty() { String::new() } else { format!(" {}", dirty_marker.trim()) };
    let commit_fact = match p.git.last_commit_at {
        Some(dt) => format!("commit {}", commit_ago(dt)),
        None => "no commits".to_string(),
    };
    let git_facts = format!("{branch}{dirty_suffix} · {commit_fact}");
    let git_sparkline = sparkline_glyphs(&p.git.daily_commits, petridish_core::schema::GIT_ACTIVITY_WINDOW_DAYS);
    let git_row = zone_row(
        ZoneRowSpec {
            indent: ZONE_INDENT,
            label: "git",
            label_style: zone_label_style(theme::BRANCH),
            facts: git_facts,
            facts_style: dim,
            sparkline: git_sparkline,
            spark_style: Style::default().fg(theme::BRANCH),
            tag: format!("{}d", petridish_core::schema::GIT_ACTIVITY_WINDOW_DAYS),
        },
        card_width,
    );

    // agent zone: agent name + session, paired with the agent-activity sparkline (one
    // sample/tick — the silence-tier gradient, same color language the header's "silent Xm"
    // already uses for this project). Width scales with the card's own allocated card_width
    // (`agent_sparkline_width_for`) rather than a fixed sample count, so a wide card genuinely
    // shows more history instead of leaving the extra card_width blank — real screen-fill, not
    // stretched decoration, since every extra sample is a real additional minute of activity
    // up to the ring's own `AGENT_ACTIVITY_WINDOW` ceiling. The git sparkline above does NOT
    // scale the same way: `GIT_ACTIVITY_WINDOW_DAYS` days is all the daily-commit history that
    // exists, so widening it further would only pad with the zero-level bar, not show more data.
    let agent_facts = match p.agent.active_agent.as_deref() {
        Some(agent) => match p.agent.session_id.as_deref() {
            Some(session) => format!("{agent} · sess {}", &session[..session.len().min(18)]),
            None => agent.to_string(),
        },
        None => "idle".to_string(),
    };
    let agent_spark_width = agent_sparkline_width_for(card_width);
    let agent_sparkline = sparkline_glyphs(&p.agent_activity, agent_spark_width);
    let agent_row = zone_row(
        ZoneRowSpec {
            indent: ZONE_INDENT,
            label: "agent",
            label_style: zone_label_style(tier_color),
            facts: agent_facts,
            facts_style: dim,
            sparkline: agent_sparkline,
            spark_style: Style::default().fg(tier_color),
            tag: format!("{agent_spark_width}m"),
        },
        card_width,
    );

    let display_path = abbreviate_home(&p.path);
    let path_row = Line::from(Span::styled(format!("{ZONE_INDENT}{display_path}"), dim));

    vec![header, git_row, agent_row, path_row]
}

/// Unicode block elements U+2581-2588 ("Block Elements") -- each a single narrow cell per
/// `unicode-width`, on petri's glyph allowlist (petri/SPEC.md §4.2, `petri/tests/glyph_portability.rs`)
/// with that reasoning checked, same rigor as every other glyph this module already renders.
const SPARKLINE_GLYPHS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Floor for the roomy-card agent sparkline's on-screen width (in samples) — what a
/// minimum-width card (`MIN_ROOMY_CARD_WIDTH`) still gets. `agent_sparkline_width_for` scales
/// this up with the card's real allocated width, capped at `AGENT_ACTIVITY_WINDOW` (there is no
/// more history than that to show).
const SPARKLINE_WIDTH: usize = 20;

/// How many agent-activity samples a roomy card's `agent` zone row should show, given its own
/// allocated `card_width` — the fixed cost of everything else on that row (indent, the
/// `ZONE_LABEL_WIDTH`-padded label, a usable facts string, the gap before the sparkline, and
/// the trailing scale tag) is subtracted first, then clamped to `[SPARKLINE_WIDTH,
/// AGENT_ACTIVITY_WINDOW]` so a narrow card still gets a legible sparkline and a very wide one
/// doesn't ask for more samples than the ring actually keeps.
fn agent_sparkline_width_for(card_width: usize) -> usize {
    let overhead = ZONE_INDENT.len() + ZONE_LABEL_WIDTH + 20 /* minimal facts */ + 2 /* gap */ + 4 /* tag */;
    card_width
        .saturating_sub(overhead)
        .clamp(SPARKLINE_WIDTH, petridish_core::schema::AGENT_ACTIVITY_WINDOW)
}

/// Renders the trailing `width` samples of `samples` (oldest first, most recent last) as a
/// compact block-glyph string. Levels are normalized against the max count *within the
/// visible window*, not the whole input, so one busy stretch that's since scrolled off
/// doesn't permanently flatten the rest of the sparkline into the lowest bar. Fewer than
/// `width` samples (a freshly-discovered project, the daemon just restarted, or -- for git --
/// a repo younger than the activity window) are left-padded with the lowest bar rather than
/// shortened, so every sparkline occupies the same on-screen width regardless of how much
/// history exists yet. Shared by both the agent-activity sparkline (`p.agent_activity`,
/// `width = SPARKLINE_WIDTH`) and the git daily-commits sparkline (`p.git.daily_commits`,
/// `width = GIT_ACTIVITY_WINDOW_DAYS`) -- same visual language, deliberately different colors
/// at the call site so the two timelines stay visually distinguishable.
fn sparkline_glyphs(samples: &[u32], width: usize) -> String {
    let start = samples.len().saturating_sub(width);
    let window = &samples[start..];
    let max = window.iter().copied().max().unwrap_or(0);
    let pad = width.saturating_sub(window.len());

    let mut out = String::with_capacity(width);
    for _ in 0..pad {
        out.push(SPARKLINE_GLYPHS[0]);
    }
    for &count in window {
        let level = if count == 0 || max == 0 {
            0
        } else {
            let scaled = (count as f64 / max as f64) * (SPARKLINE_GLYPHS.len() - 1) as f64;
            (scaled.round() as usize).clamp(1, SPARKLINE_GLYPHS.len() - 1)
        };
        out.push(SPARKLINE_GLYPHS[level]);
    }
    out
}

/// Compact row for a project in IN FLIGHT / STALE / COLD.
/// `show_git_sparkline` is true only for IN FLIGHT (see `plan_layout`'s `shows_git_sparkline`
/// doc comment for the 14-day alignment reasoning) — STALE/COLD rows omit it and keep the
/// original plain fields.
fn compact_row_line(radar: &Radar, proj_idx: usize, is_selected: bool, card_width: usize, show_git_sparkline: bool) -> Line<'static> {
    let p = &radar.projects[proj_idx];
    let branch = p.git.branch.as_deref().unwrap_or("(none)");
    let uncommitted = if p.git.uncommitted_files > 0 { format!("✎{}", p.git.uncommitted_files) } else { String::new() };
    let commit_age = match p.git.last_commit_at {
        Some(dt) => commit_ago(dt),
        None => "(none)".to_string(),
    };
    let gh = if p.git.github_url.is_some() { "[gh]" } else { "" };

    let name_field = format!(" {} ", fixed_width(&p.name, 26));
    let branch_field = format!("{} ", fixed_width(branch, 22));
    let uncommitted_field = format!("{} ", fixed_width(&uncommitted, 4));
    let commit_field = format!("{commit_age} ");

    let (sparkline, tag) = if show_git_sparkline {
        (
            sparkline_glyphs(&p.git.daily_commits, petridish_core::schema::GIT_ACTIVITY_WINDOW_DAYS),
            format!("{}d", petridish_core::schema::GIT_ACTIVITY_WINDOW_DAYS),
        )
    } else {
        (String::new(), String::new())
    };

    let left = format!("{name_field}{branch_field}{uncommitted_field}{commit_field}{gh}");

    if is_selected {
        let content = if show_git_sparkline {
            let pad = card_width
                .saturating_sub(left.chars().count() + sparkline.chars().count() + 2 + tag.chars().count())
                .max(1);
            format!("{left}{}{sparkline}  {tag}", " ".repeat(pad))
        } else {
            left
        };
        return solid_selected_line(&content, card_width);
    }

    let mut spans = vec![
        Span::styled(name_field, Style::default().fg(theme::FG)),
        Span::styled(branch_field, Style::default().fg(theme::BRANCH)),
        Span::styled(uncommitted_field, Style::default().fg(theme::DIM)),
        Span::styled(commit_field, Style::default().fg(theme::DIM)),
        Span::styled(gh, Style::default().fg(theme::ACCENT)),
    ];

    if show_git_sparkline {
        let pad = card_width
            .saturating_sub(left.chars().count() + sparkline.chars().count() + 2 + tag.chars().count())
            .max(1);
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(sparkline, Style::default().fg(theme::BRANCH)));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(tag, Style::default().fg(theme::DIM)));
    }

    Line::from(spans)
}

/// A selected compact row's highlight: one solid, continuously-colored bar across the row's
/// full `card_width`, instead of per-field background fills that leave unstyled gaps wherever a
/// field's own text doesn't reach its column width (that ragged-highlight bug is what this
/// replaces — see `ROOMY_CARD_BOX_ROWS`'s doc comment for the equivalent fix on roomy cards).
/// This is the reverse-video convention applied literally: field-level color is deliberately
/// dropped in favor of one unambiguous highlighted bar, matching "reverse video is the canonical
/// current-selection signal" (`references/visual-patterns.md` → *Typography in monospace*).
fn solid_selected_line(content: &str, card_width: usize) -> Line<'static> {
    Line::from(Span::styled(
        fixed_width(content, card_width),
        Style::default().fg(Color::Black).bg(theme::ACCENT).add_modifier(Modifier::BOLD),
    ))
}

/// Footer: the keymap (petri/SPEC.md §5 — honest and useful, not a fixed list).
fn footer_line() -> Line<'static> {
    Line::from(Span::styled(
        " j/k move  Space toggle  Enter open/browser  Tab Browser  q quit ",
        Style::default().fg(theme::DIM),
    ))
}

/// Format a past timestamp as `"Xs ago"` / `"Xm ago"` / `"Xh ago"` / `"Xd ago"`.
/// Future timestamps (a clock-skew edge case, not expected in practice) clamp
/// to zero rather than printing a negative duration.
fn commit_ago(dt: chrono::DateTime<chrono::Utc>) -> String {
    let secs = chrono::Utc::now().signed_duration_since(dt).num_seconds().max(0);
    format!("{} ago", humanize_secs(secs as u64))
}

/// Format a duration in seconds to "3m", "1h", "6d", or "Xs".
fn humanize_secs(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

/// Abbreviate `$HOME/...` to `~/...` for display. Returns the input unchanged
/// when `$HOME` is unset or the path doesn't start with it.
fn abbreviate_home(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME") {
        if let Ok(p) = std::path::Path::new(path).strip_prefix(&home) {
            let remainder = p.to_string_lossy().to_string();
            if remainder.is_empty() {
                return home;
            }
            let sep = if remainder.starts_with('/') { "" } else { "/" };
            return format!("~{sep}{remainder}");
        }
    }
    path.to_string()
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use petridish_core::schema::{AgentState, GitState};

    fn project(id: &str, bucket: StatusBucket) -> Project {
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
            status_bucket: bucket,
            agent_activity: Vec::new(),
        }
    }

    fn radar_with(n_active: usize) -> Radar {
        radar_with_bucket(n_active, StatusBucket::Active)
    }

    fn radar_with_bucket(n: usize, bucket: StatusBucket) -> Radar {
        let projects = (0..n).map(|i| project(&format!("p{i}"), bucket)).collect();
        Radar {
            schema_version: 1,
            updated_at: chrono::Utc::now(),
            scan_duration_ms: 0,
            projects,
            quota: None,
        }
    }

    const EXPANDED: CollapsedState = [false, false, true, true];

    #[test]
    fn narrow_terminal_stays_single_column() {
        let radar = radar_with(6);
        let plan = plan_layout(Rect::new(0, 0, 80, 40), &radar, EXPANDED);
        assert_eq!(plan.sections[0].columns, 1, "80 cols is below 2×MIN_ROOMY_CARD_WIDTH, so no second column");
    }

    #[test]
    fn wide_terminal_grows_columns_up_to_the_cap() {
        let radar = radar_with(20);
        let plan = plan_layout(Rect::new(0, 0, 200, 60), &radar, EXPANDED);
        assert_eq!(plan.sections[0].columns, 3, "200 cols fits 3× MIN_ROOMY_CARD_WIDTH+gutter but not 4");

        let plan = plan_layout(Rect::new(0, 0, 400, 60), &radar, EXPANDED);
        assert_eq!(plan.sections[0].columns, MAX_GRID_COLUMNS, "an very wide terminal must still respect the column cap");
    }

    #[test]
    fn grid_rows_account_for_multiple_columns_not_one_row_per_item() {
        let radar = radar_with(6);
        // 3 columns, 6 items -> 2 grid rows, not 6.
        let plan = plan_layout(Rect::new(0, 0, 200, 60), &radar, EXPANDED);
        let section = &plan.sections[0];
        assert_eq!(section.columns, 3);
        assert_eq!(section.items_shown, 6);
        assert_eq!(section.grid_rows, 2, "6 items across 3 columns must take 2 grid rows");
    }

    #[test]
    fn compact_section_stays_single_column_when_it_already_fits_a_wide_terminal() {
        // Feedback (dashboard redesign follow-up): compact rows (IN FLIGHT/STALE/COLD) don't
        // get richer with a wider column the way a roomy card does, so gridding one wider than
        // necessary just fragments a section that would otherwise read as one clean list —
        // especially wasteful when there's a full page of blank space below it regardless. 5
        // IN FLIGHT rows in a very tall, very wide terminal must stay 1 column even though the
        // width alone would allow several.
        let radar = radar_with_bucket(5, StatusBucket::InFlight);
        let plan = plan_layout(Rect::new(0, 0, 300, 200), &radar, EXPANDED);
        let section = &plan.sections[0];
        assert_eq!(section.bucket, StatusBucket::InFlight);
        assert_eq!(section.columns, 1, "5 rows already fit in 1 column with room to spare, so no grid is needed");
        assert_eq!(section.items_shown, 5);
        assert!(section.truncated_remaining.is_none());
    }

    #[test]
    fn compact_section_still_grids_when_a_single_column_would_truncate() {
        // The needs-driven column count must still reach for more columns when 1 genuinely
        // isn't enough to show everything in the available height.
        let radar = radar_with_bucket(60, StatusBucket::InFlight);
        let plan = plan_layout(Rect::new(0, 0, 300, 20), &radar, EXPANDED);
        let section = &plan.sections[0];
        assert!(section.columns > 1, "60 rows cannot fit in 1 column within a 20-row terminal, so it must grid");
    }

    #[test]
    fn roomy_section_grids_even_when_a_single_column_would_already_fit() {
        // Contrast with the compact-row case above: RUNNING's roomy cards get real extra
        // information from a wider column (the agent sparkline scales up), so they always claim
        // the width-driven column count — unlike compact rows, going wider isn't wasted here.
        let radar = radar_with(3);
        let plan = plan_layout(Rect::new(0, 0, 200, 200), &radar, EXPANDED);
        let section = &plan.sections[0];
        assert!(section.columns > 1, "3 roomy cards easily fit in 1 column at this height, but width should still be used");
    }

    #[test]
    fn truncation_still_fires_in_a_grid_and_names_the_remaining_count() {
        // A short terminal with many projects: even a multi-column grid must truncate rather
        // than overflow, and say how many were cut.
        let radar = radar_with(200);
        let plan = plan_layout(Rect::new(0, 0, 200, 12), &radar, EXPANDED);
        let section = &plan.sections[0];
        assert!(section.items_shown < 200, "must truncate rather than render all 200 projects into 12 rows");
        assert_eq!(
            section.truncated_remaining,
            Some(200 - section.items_shown),
            "the marker's remaining count must match what was actually cut"
        );
    }

    #[test]
    fn does_not_panic_at_degenerate_and_pinned_sizes() {
        let radar = radar_with(10);
        for (w, h) in [(0, 0), (1, 1), (80, 24), (200, 50), (400, 100)] {
            let _ = plan_layout(Rect::new(0, 0, w, h), &radar, EXPANDED);
        }
    }
}

#[cfg(test)]
mod sparkline_tests {
    use super::*;

    #[test]
    fn empty_ring_renders_all_lowest_bars() {
        let out = sparkline_glyphs(&[], SPARKLINE_WIDTH);
        assert_eq!(out.chars().count(), SPARKLINE_WIDTH);
        assert!(out.chars().all(|c| c == SPARKLINE_GLYPHS[0]));
    }

    #[test]
    fn all_zero_ring_renders_all_lowest_bars_not_a_flat_high_bar() {
        let ring = vec![0u32; SPARKLINE_WIDTH];
        let out = sparkline_glyphs(&ring, SPARKLINE_WIDTH);
        assert!(
            out.chars().all(|c| c == SPARKLINE_GLYPHS[0]),
            "an all-zero window must render as the lowest bar throughout, got {out:?}"
        );
    }

    #[test]
    fn fewer_than_width_samples_are_left_padded_with_the_lowest_bar() {
        // Three real samples, all with the same nonzero count -> the rightmost three
        // chars are the max-level bar, everything to their left is left-pad.
        let ring = vec![5u32, 5, 5];
        let out: Vec<char> = sparkline_glyphs(&ring, SPARKLINE_WIDTH).chars().collect();
        assert_eq!(out.len(), SPARKLINE_WIDTH);
        let pad = SPARKLINE_WIDTH - 3;
        assert!(
            out[..pad].iter().all(|&c| c == SPARKLINE_GLYPHS[0]),
            "left pad must be the lowest bar: {out:?}"
        );
        assert!(
            out[pad..].iter().all(|&c| c == SPARKLINE_GLYPHS[SPARKLINE_GLYPHS.len() - 1]),
            "the three equal, nonzero, max-of-window samples must render at the top bar: {out:?}"
        );
    }

    #[test]
    fn only_the_trailing_width_samples_are_shown() {
        // A ring longer than SPARKLINE_WIDTH: the sparkline must reflect only the last
        // SPARKLINE_WIDTH samples, not the whole ring (older samples fall off the left).
        let mut ring = vec![0u32; SPARKLINE_WIDTH * 2];
        // Put a lone spike just before the visible window -- must NOT show up.
        ring[SPARKLINE_WIDTH - 1] = 999;
        // And a spike inside the visible window -- must show up as the max bar.
        let visible_spike_idx = ring.len() - 1;
        ring[visible_spike_idx] = 5;

        let out: Vec<char> = sparkline_glyphs(&ring, SPARKLINE_WIDTH).chars().collect();
        assert_eq!(out.len(), SPARKLINE_WIDTH);
        assert_eq!(
            *out.last().unwrap(), SPARKLINE_GLYPHS[SPARKLINE_GLYPHS.len() - 1],
            "the in-window spike must render as the top bar (it's the window's own max): {out:?}"
        );
        // Nothing else in the visible window is nonzero, so everything but the last char
        // must be the lowest bar -- the off-window spike must have no visible effect.
        assert!(
            out[..out.len() - 1].iter().all(|&c| c == SPARKLINE_GLYPHS[0]),
            "an off-window spike must not influence the visible normalization: {out:?}"
        );
    }

    #[test]
    fn normalizes_relative_to_the_windows_own_max_not_a_fixed_scale() {
        // Two samples: half of max should land roughly mid-scale, not pinned to a fixed
        // absolute-count threshold.
        let mut ring = vec![0u32; SPARKLINE_WIDTH - 2];
        ring.push(2); // half of the window's max
        ring.push(4); // the window's max -> must render as the top bar
        let out: Vec<char> = sparkline_glyphs(&ring, SPARKLINE_WIDTH).chars().collect();
        assert_eq!(*out.last().unwrap(), SPARKLINE_GLYPHS[SPARKLINE_GLYPHS.len() - 1]);
        let half_level = out[out.len() - 2];
        assert_ne!(
            half_level, SPARKLINE_GLYPHS[0],
            "a nonzero count must never render as the zero-count bar: {out:?}"
        );
        assert_ne!(
            half_level, SPARKLINE_GLYPHS[SPARKLINE_GLYPHS.len() - 1],
            "half of the window's max should not render identically to the max itself: {out:?}"
        );
    }

    #[test]
    fn zone_row_places_label_facts_and_sparkline_in_order_with_the_tag_last() {
        let line = zone_row(
            ZoneRowSpec {
                indent: "  ",
                label: "git",
                label_style: Style::default(),
                facts: "FACTS".to_string(),
                facts_style: Style::default(),
                sparkline: "SPARK".to_string(),
                spark_style: Style::default(),
                tag: "14d".to_string(),
            },
            60,
        );
        let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let label_pos = rendered.find("git").unwrap();
        let facts_pos = rendered.find("FACTS").unwrap();
        let spark_pos = rendered.find("SPARK").unwrap();
        let tag_pos = rendered.find("14d").unwrap();
        assert!(label_pos < facts_pos, "label must precede facts: {rendered:?}");
        assert!(facts_pos < spark_pos, "facts must precede the sparkline: {rendered:?}");
        assert!(spark_pos < tag_pos, "the sparkline must precede its scale tag: {rendered:?}");
    }

    #[test]
    fn zone_row_never_panics_when_content_overflows_width() {
        // facts+sparkline longer than width -- must degrade to a wider-than-terminal line
        // (ratatui clips at render time), never panic on an underflowing subtraction.
        let line = zone_row(
            ZoneRowSpec {
                indent: "  ",
                label: "agent",
                label_style: Style::default(),
                facts: "x".repeat(30),
                facts_style: Style::default(),
                sparkline: "y".repeat(30),
                spark_style: Style::default(),
                tag: "z".repeat(30),
            },
            10,
        );
        let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(rendered.contains('x') && rendered.contains('y') && rendered.contains('z'));
    }
}
