//! The S4 walking-skeleton screen: header + flat list of every project, no
//! grouping/selection/detail pane yet (those are S5's Browser and S6's
//! Dashboard). Kept deliberately trivial in content — S4's entire job is
//! retiring the infrastructure risk (terminal setup, event loop, panic hook,
//! snapshot/PTY harnesses), not the screen itself (petri/SPEC.md §9).

use petridish_core::schema::Radar;
use ratatui::Frame;

/// Render the header + flat project list into `frame`.
///
/// Contract asserted by `petri/tests/s4_snapshot.rs` (structural, not an exact
/// buffer — see that file's module doc comment for why):
/// - The top line (row 0) contains the literal substring `"petri"` somewhere in
///   it (the eventual "petri · dashboard"/"petri · browser" header land in S6/S7;
///   S4 just needs *a* header identifying the app).
/// - Every project in `radar.projects` has its `name` appear as a substring
///   somewhere in the rendered buffer, in project order (top to bottom), for as
///   many projects as fit in the given height. Exact column layout, padding, and
///   truncation behavior for names that don't fit are your call.
/// - Must not panic on an empty `radar.projects` (zero rows is a valid state —
///   the "missing state file" case is handled before this function is ever
///   called; an empty *parsed* file is different and must render, not crash).
pub fn render(frame: &mut Frame, radar: &Radar) {
    todo!("S4: header + flat list rendering")
}
