//! The Browser screen (S5): grouped project list, selection, detail pane,
//! type-ahead filter. petri/SPEC.md §3.1 is the authoritative behavior
//! contract — read it in full before touching this file.

use petridish_core::schema::{Project, Radar, StatusBucket};
use ratatui::Frame;

/// Section order (petri/SPEC.md §3.1): "Grouped list, sections in the fixed
/// order active, in_flight, stale, cold". No headers in the Browser (v1) —
/// that's a Dashboard-only concept (S6) — so this is purely a sort key.
pub const SECTION_ORDER: [StatusBucket; 4] =
    [StatusBucket::Active, StatusBucket::InFlight, StatusBucket::Stale, StatusBucket::Cold];

/// Browser state: which projects are visible (grouped, filtered, excluding
/// `is_foreign`) and which one is selected.
pub struct BrowserState {
    /// Indices into the `Radar.projects` slice passed to `new`/`apply_filter`,
    /// in section order (`SECTION_ORDER`), already excluding `is_foreign`
    /// projects and anything not matching the current filter. This is the
    /// Browser's visible row list — `selected` indexes into THIS list
    /// (a position), not into `Radar.projects` directly.
    pub visible: Vec<usize>,
    /// Position within `visible`, or `None` when `visible` is empty. Must
    /// never be an out-of-bounds index into `visible` — the empty selection
    /// must be representable without panicking anywhere that reads it.
    pub selected: Option<usize>,
    /// The current type-ahead filter query (petri/SPEC.md §3.1 "`/` opens a
    /// type-ahead filter"). Empty string = unfiltered.
    pub filter_query: String,
}

impl BrowserState {
    /// Build the initial state from `radar`: exclude `is_foreign` projects,
    /// group by `SECTION_ORDER`, select the first visible row (or `None` if
    /// there are no visible projects at all). Unfiltered (`filter_query` is
    /// empty).
    pub fn new(radar: &Radar) -> Self {
        todo!("S5")
    }

    /// Move the selection by `delta` (negative = up, positive = down) within
    /// `visible`, **clamped at both ends — never wrapping**. A `delta` that
    /// would go past the last row stops AT the last row, not back to zero (and
    /// symmetrically at the top). No-op (and does not panic) when `visible` is
    /// empty.
    pub fn move_selection(&mut self, delta: i32) {
        todo!("S5")
    }

    /// Re-derive `visible` from `radar`, filtered by `query`
    /// (case-insensitive substring match against `Project.name`; an empty
    /// `query` returns the full unfiltered, grouped list — petri/SPEC.md
    /// §3.1 "an empty query returns the input unchanged"). Also updates
    /// `filter_query` to `query`.
    ///
    /// If the project that was selected before this call is still present in
    /// the new `visible`, selection follows it (stays on the same project,
    /// even if its position in `visible` changed). Otherwise selection resets
    /// to the first available row (`Some(0)`), or `None` if the new `visible`
    /// is empty. Must not panic in either case.
    pub fn apply_filter(&mut self, radar: &Radar, query: &str) {
        todo!("S5")
    }

    /// The currently selected project, if any.
    pub fn selected_project<'a>(&self, radar: &'a Radar) -> Option<&'a Project> {
        todo!("S5")
    }
}

/// Render the Browser screen: grouped list (section headers + counts, per
/// `SECTION_ORDER`) on the left, detail pane on the right per petri/SPEC.md
/// §3.1. Section headers are NOT selection stops in the Browser (that's a
/// Dashboard-only, S6 concept) — they're rendered, just not part of
/// `state.visible`.
///
/// Contract asserted by `petri/tests/s5_snapshot.rs` (structural, same
/// approach as S4's `app::render` — see that file's module doc comment):
/// - Row 0 contains `"petri"`.
/// - Every project in `state.visible` (i.e. every non-foreign, filter-passing
///   project) has its name appear somewhere in the rendered output, for as
///   many as fit — exact layout/truncation is your call.
/// - At a narrow enough width, the detail pane must be **absent entirely**,
///   never squeezed into an unreadable sliver (petri/SPEC.md §3.1 "If the
///   window is too narrow to give it a usable width, hide it entirely rather
///   than squeezing").
/// - Must not panic on an empty `state.visible` (renders a "nothing
///   selected" state per petri/SPEC.md §3.1) or a degenerate 0×0/1×1 area.
pub fn render(frame: &mut Frame, radar: &Radar, state: &BrowserState) {
    todo!("S5")
}
