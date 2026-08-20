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

use petridish_core::schema::{Project, Radar, StatusBucket};

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
    Header(StatusBucket),
    /// Index into `radar.projects`.
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

impl DashboardState {
    /// Build the initial state from `radar`: default collapse state (`STALE`
    /// and `COLD` collapsed), cursor on the first stop (`RUNNING`'s header,
    /// or `None` if there is nothing to show at all — see
    /// `does_not_panic_on_all_empty_radar` in `s6_dashboard.rs`).
    pub fn new(_radar: &Radar) -> Self {
        todo!("S6: DashboardState::new")
    }

    /// Membership for the `RUNNING` (Active) section per ADR-0001: a project
    /// counts as running if its own `status_bucket` is `Active`, OR it has at
    /// least one worktree child (`parent_path == Some(this project's path)`)
    /// whose own `status_bucket` is `Active`. `is_foreign` projects are
    /// always excluded (same convention as `BrowserState::new`). Display-only
    /// — never mutates `status_bucket` (ADR-0001).
    ///
    /// Ordered **quietest first**: the project with the oldest
    /// `last_activity_at` (or `None`, treated as "never active" / maximally
    /// silent, sorted before any `Some`) comes first — petri/SPEC.md §3.2:
    /// "the stalled run is the one that needs you".
    pub fn running_membership(_radar: &Radar) -> Vec<usize> {
        todo!("S6: DashboardState::running_membership")
    }

    /// Recompute `visible` (and clamp/carry `selected`) from `radar` and the
    /// current `collapsed` state. Called by `new` and after any toggle.
    fn rebuild(&mut self, _radar: &Radar) {
        todo!("S6: DashboardState::rebuild")
    }

    /// Move the cursor by `delta` stops in `visible`, clamped at both ends,
    /// never wrapping. No-op on an empty `visible`.
    pub fn move_selection(&mut self, _delta: i32) {
        todo!("S6: DashboardState::move_selection")
    }

    /// `Space` (or `Enter` on a header) semantics. If the current stop is a
    /// `Header`, toggle that section's collapse state. If the current stop is
    /// a `Project` row, toggle the collapse state of the section that row
    /// belongs to, then move selection to that section's (now-toggled)
    /// header — the cursor must never be left pointing at a row that just
    /// stopped existing.
    pub fn toggle_selected(&mut self, _radar: &Radar) {
        todo!("S6: DashboardState::toggle_selected")
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

/// Render the Dashboard into `frame`. Per petri/SPEC.md §3.2:
/// - Header: `petri · dashboard`, project count, clock, last scan duration.
/// - `RUNNING` section: roomy cards (glyph, name, `silent 3m · claude-code`,
///   branch, dirty marker, `commit 1d ago (you)`, `~`-abbreviated path,
///   truncated session id); worktree nesting under an in-section parent, or
///   the `name (in parent-name)` suffix form when the parent isn't in this
///   section.
/// - `IN FLIGHT`/`STALE`/`COLD`: compact rows (name, branch, `✎N`, commit
///   age, `gh` marker); a parent with worktree children shows
///   `name · N worktrees` instead of listing them.
/// - Collapsed sections still render their header + count, but no rows.
/// - **Overflow: truncate, never scroll.** If expanded sections exceed the
///   available height, sections emit in priority order (`SECTION_ORDER`) and
///   stop, with a required `… +N more` marker at the cut.
/// - Staleness banner when `radar.updated_at` is older than 24h.
/// - Must not panic on an empty `radar.projects`, nor at 0×0 or 1×1.
pub fn render(frame: &mut ratatui::Frame, _radar: &Radar, _state: &DashboardState) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    todo!("S6: dashboard::render")
}

#[cfg(test)]
mod tests {
    // Pure-state contract tests live in `petri/tests/s6_dashboard.rs` (the
    // orchestrator-authored acceptance gate) — nothing here yet.
}
