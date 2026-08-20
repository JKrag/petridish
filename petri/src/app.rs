//! The S4 walking-skeleton screen: header + flat list of every project, no
//! grouping/selection/detail pane yet (those are S5's Browser and S6's
//! Dashboard). Kept deliberately trivial in content — S4's entire job is
//! retiring the infrastructure risk (terminal setup, event loop, panic hook,
//! snapshot/PTY harnesses), not the screen itself (petri/SPEC.md §9).

use petridish_core::schema::Radar;
use ratatui::{
    layout::{Constraint, Layout},
    style::Style,
    text::Line,
    widgets::{Paragraph, Wrap},
    Frame,
};

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
    let area = frame.area();

    // Guard against 0×0 (a freshly-forked pty reports these; ratatui handles
    // them on its own, but this explicit check avoids any surprise deep inside
    // the widget renderers).
    if area.width == 0 || area.height == 0 {
        return;
    }

    // Top row: application header. Later slices replace the trailing label
    // with `"petri · dashboard"` / `"petri · browser"` per SPEC §3 — for S4,
    // the contract is simply that `"petri"` appears somewhere in row 0.
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);

    let header = Paragraph::new(Line::from("petri · walking skeleton"))
        .style(Style::default().bold())
        .wrap(Wrap { trim: false });
    frame.render_widget(header, chunks[0]);

    // Flat project list — one paragraph line per project. A single
    // `Paragraph` is enough and it handles both empty and overflow layouts
    // without panicking (ratatui's widget pipeline clamps negative sizes to
    // zero rather than dividing by zero).
    let body_lines: Vec<Line> = radar
        .projects
        .iter()
        .map(|p| Line::from(p.name.clone()))
        .collect();

    let list = Paragraph::new(body_lines).wrap(Wrap { trim: false });
    frame.render_widget(list, chunks[1]);
}
