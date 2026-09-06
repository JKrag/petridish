//! The `?` help popup (`petri/IDEAS.md` ACT-2, `MECH-1`'s second customer).
//!
//! Pure content, no interaction beyond "any key closes it" — unlike
//! `picker.rs` there is no cursor, no selection, nothing to choose. The
//! action-key half of the list is generated from `tools::registry()`, per
//! this codebase's stated norm that actions are data, not hardcoded keys
//! (see lib.rs's action-key match arm comment). `y` and `?` themselves are
//! NOT registry entries, so they are the one deliberate hardcoded exception
//! below.

/// Draw the help popup as a centred overlay (`MECH-1`), same technique as
/// `picker::render`: `Clear` the region, then draw a bordered `Paragraph`
/// over it. Must be called last in the frame, same as the picker.
pub fn render(frame: &mut ratatui::Frame) {
    use ratatui::layout::{Constraint, Flex, Layout};
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph};

    let registry = crate::tools::registry();

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        "Navigation",
        Style::default()
            .fg(crate::theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    )));
    for (key, label) in [
        ("j/k, ↑/↓", "move selection"),
        ("J/K", "fast jump (~10 rows)"),
        ("PageUp/PageDown", "jump one screenful"),
        ("Home/End", "jump to first/last row"),
        ("/", "filter"),
        ("Tab", "switch to Dashboard"),
        ("q", "quit"),
    ] {
        lines.push(Line::from(format!("  {key:<16} {label}")));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Actions",
        Style::default()
            .fg(crate::theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    )));
    for action in &registry {
        lines.push(Line::from(format!(
            "  {:<16} {} (Shift+{} to re-pick)",
            action.key,
            action.label,
            action.key.to_ascii_uppercase()
        )));
    }
    // Not registry entries — the one deliberate hardcoded exception, see the
    // module doc comment.
    lines.push(Line::from("  y                yank path to clipboard"));
    lines.push(Line::from("  ?                this help"));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "any key closes this popup",
        Style::default().fg(crate::theme::DIM),
    )));

    let area = frame.area();
    let width = 62.min(area.width.saturating_sub(4)).max(20);
    let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let [popup] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(popup);

    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(crate::theme::ACCENT))
        .title(Span::styled(
            " help ",
            Style::default()
                .fg(crate::theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(lines), inner);
}
