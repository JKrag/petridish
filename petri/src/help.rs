//! The `?` help popup (`petri/IDEAS.md` ACT-2, `MECH-1`'s second customer).
//!
//! Pure content, no interaction beyond "any key closes it" — unlike
//! `picker.rs` there is no cursor, no selection, nothing to choose. The
//! action-key half of the list is generated from `tools::registry()`, per
//! this codebase's stated norm that actions are data, not hardcoded keys
//! (see lib.rs's action-key match arm comment). `y` and `?` themselves are
//! NOT registry entries, so they are the one deliberate hardcoded exception
//! below.
//!
//! **The Navigation half is screen-specific; the Actions half is not.** Every registry
//! action already fires on both the Dashboard and the Browser (issue #64), but their
//! navigation keys genuinely differ — `J`/`K`/`PageUp`/`PageDown`/`Home`/`End`/`/` are
//! Browser-only (`SPEC.md` §5: the Dashboard's "truncate, never scroll" model has no
//! viewport to page through), and `y` is Browser-only for an unrelated reason (it yanks
//! the Browser's own row selection, never wired to the Dashboard's). Showing the
//! Browser's list on the Dashboard would advertise half a dozen keys that do nothing
//! there — exactly the "would a new user be misled?" failure `SPEC.md` §5.1's footer
//! rule already forbids for the footer, applied here to the popup that exists to answer
//! "what can I press."

/// Draw the help popup as a centred overlay (`MECH-1`), same technique as
/// `picker::render`: `Clear` the region, then draw a bordered `Paragraph`
/// over it. Must be called last in the frame, same as the picker.
///
/// `screen` selects the Navigation half's content (see the module doc comment); the
/// Actions half is identical either way.
pub fn render(frame: &mut ratatui::Frame, screen: crate::Screen) {
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
    let nav: &[(&str, &str)] = match screen {
        crate::Screen::Browser => &[
            ("j/k, ↑/↓", "move selection"),
            // Placed right after the most-used binding, not at the list's tail:
            // `render`'s height clamp (below) can clip this popup's own content
            // on a short terminal, and a narrow-AND-short terminal — exactly the
            // geometry where `Space` is the only route to the detail pane's
            // fields (issue #35) — is also the geometry most likely to clip it.
            ("Space", "toggle detail popup (narrow/short terminals)"),
            ("J/K", "fast jump (~10 rows)"),
            ("PageUp/PageDown", "jump one screenful"),
            ("Home/End", "jump to first/last row"),
            ("/", "filter"),
            ("Tab", "switch to Dashboard"),
            ("q", "quit"),
        ],
        crate::Screen::Dashboard => &[
            ("j/k, ↑/↓", "move selection"),
            (
                "Space",
                "collapse/expand a section, or open a project's details",
            ),
            (
                "Enter",
                "toggle a section, or jump to the Browser with this project",
            ),
            ("Esc", "close the project popup"),
            ("Tab", "switch to Browser"),
            ("q", "quit"),
        ],
    };
    for (key, label) in nav {
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
    // Not a registry entry, and Browser-only (see the module doc comment) — the one
    // deliberate hardcoded exception, and only on the screen where it actually fires.
    if screen == crate::Screen::Browser {
        lines.push(Line::from("  y                yank path to clipboard"));
    }
    lines.push(Line::from("  ?                this help"));

    // The closing hint is drawn in its own fixed row rather than appended to `lines`, so it
    // stays visible even when the registry has grown too tall for a short terminal to show
    // the whole list. `registry()` gained a seventh entry (`u`, #29) without this repo's
    // help popup gaining a row to spend on it, which used to push this hint off the bottom
    // of an 80x24 terminal along with the last entry or two of `Actions` — a Paragraph
    // clips, it does not scroll. Pinning the hint keeps that failure mode to "an action or
    // two got clipped" instead of "the popup forgot to say how to close itself".
    let footer = Line::from(Span::styled(
        "any key closes this popup",
        Style::default().fg(crate::theme::DIM),
    ));

    let area = frame.area();
    let width = 62.min(area.width.saturating_sub(4)).max(20);
    // +2 for the block border, +1 for the footer row — clamped to the terminal's own height
    // so the popup itself never overflows the frame.
    let height = (lines.len() as u16 + 3).min(area.height.saturating_sub(2));
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

    let [body, footer_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    frame.render_widget(Paragraph::new(lines), body);
    frame.render_widget(Paragraph::new(footer), footer_area);
}
