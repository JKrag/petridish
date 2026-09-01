//! The Browser screen (S5): grouped project list, selection, detail pane,
//! type-ahead filter. petri/SPEC.md §3.1 is the authoritative behavior
//! contract — read it in full before touching this file.

use crate::theme;
use petridish_core::present as present;
use petridish_core::schema::{AgentActivity, Project, Radar, StatusBucket};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
    Frame,
};

use std::time::{SystemTime, UNIX_EPOCH};

/// Section order (petri/SPEC.md §3.1): "Grouped list, sections in the fixed
/// order active, in_flight, stale, cold". The Browser DOES render headers
/// with counts (§3.1) — they are just not selection stops here (selection
/// only ever lands on a project row; `BrowserState.visible` holds project
/// indices only, never a header). This is a Dashboard-only distinction (S6):
/// the Dashboard's headers ARE stops (§3.2), which needs its own cursor type
/// — do not reuse `BrowserState` for it. See browser::render for how a
/// header is drawn without being reachable by move_selection.
pub const SECTION_ORDER: [StatusBucket; 4] =
    [StatusBucket::Active, StatusBucket::InFlight, StatusBucket::Stale, StatusBucket::Cold];

/// Section header labels per spec §3.1, in the same order as `SECTION_ORDER`.
/// The Dashboard's collapsed-section headers share these values (S6).
const SECTION_LABELS: [(StatusBucket, &str); 4] = [
    (StatusBucket::Active, "RUNNING"),
    (StatusBucket::InFlight, "IN FLIGHT"),
    (StatusBucket::Stale, "STALE"),
    (StatusBucket::Cold, "COLD"),
];

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
        let visible = grouped_visible_indices(radar, "");
        let selected = if visible.is_empty() { None } else { Some(0) };
        Self { visible, selected, filter_query: String::new() }
    }

    /// Move the selection by `delta` (negative = up, positive = down) within
    /// `visible`, **clamped at both ends — never wrapping**. A `delta` that
    /// would go past the last row stops AT the last row, not back to zero (and
    /// symmetrically at the top). No-op (and does not panic) when `visible` is
    /// empty. `delta` is added via `saturating_add` (not plain `+`) so the
    /// Home/End jump-to-edge bindings (`lib.rs`, `i32::MIN`/`i32::MAX`) can't
    /// overflow — `current + i32::MAX` would panic in a debug build otherwise.
    pub fn move_selection(&mut self, delta: i32) {
        if self.visible.is_empty() {
            return;
        }
        let n = self.visible.len() as i32;
        let current = self.selected.unwrap_or(0) as i32;
        self.selected = Some(current.saturating_add(delta).clamp(0, n - 1) as usize);
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
        let previously_selected_project = self.selected.and_then(|pos| {
            self.visible.get(pos).copied()
        });

        let new_visible = grouped_visible_indices(radar, query);
        self.filter_query = query.to_string();
        self.visible = new_visible;

        match previously_selected_project {
            Some(idx) => {
                if let Some(new_pos) = self.visible.iter().position(|&v| v == idx) {
                    self.selected = Some(new_pos);
                } else if self.visible.is_empty() || self.selected.is_none() {
                    // Project was filtered out — reset to first row, or stay
                    // None if the new list is empty too.
                    self.selected = if self.visible.is_empty() { None } else { Some(0) };
                }
            }
            None => {
                // Previously-empty selection — keep it None when the new list is
                // also empty, or select row 0 otherwise.
                if self.visible.is_empty() {
                    // stay None (already set).
                } else {
                    self.selected = Some(0);
                }
            }
        }
    }

    /// The currently selected project, if any.
    pub fn selected_project<'a>(&self, radar: &'a Radar) -> Option<&'a Project> {
        let pos = self.selected?;
        let proj_idx = *self.visible.get(pos)?;
        radar.projects.get(proj_idx)
    }
}

/// Build the project list indices ordered by `SECTION_ORDER`, optionally
/// filtered by a case-insensitive substring match on `Project.name`. Foreign
/// projects are always excluded. An empty query preserves all non-foreign
/// entries in their section order.
fn grouped_visible_indices(radar: &Radar, query: &str) -> Vec<usize> {
    let mut result = Vec::with_capacity(radar.projects.len());
    for bucket in &SECTION_ORDER {
        for (idx, project) in radar.projects.iter().enumerate() {
            if project.is_foreign {
                continue;
            }
            if project.status_bucket != *bucket {
                continue;
            }
            if !query.is_empty()
                && !project.name.to_lowercase().contains(&query.to_lowercase())
            {
                continue;
            }
            result.push(idx);
        }
    }
    result
}

/// Width threshold (inclusive): the detail pane is shown whenever the full
/// terminal width is at least this many columns. 65 leaves ~40 cols for the
/// detail pane after the list takes ~25, which is enough to read path + branch
/// + one field per row without squeezing — matching petri/SPEC.md §3.1's
/// "If the window is too narrow to give it a usable width, hide it entirely."
/// The acceptance test asserts this at 40 cols (must be absent), so any value
/// > 40 is acceptable; 65 gives a generous margin.
const DETAIL_PANE_THRESHOLD: u16 = 65;

/// Minimum usable content width for the detail pane (after subtracting the
/// scrollbar column). Below this we treat the detail pane as non-usable and
/// suppress it. 25 cols is plenty for path + one short field per row.
const DETAIL_PANE_DETAIL_MIN: u16 = 25;

/// Render the Browser screen: grouped list (section headers + counts, per
/// `SECTION_ORDER`) on the left, detail pane on the right per petri/SPEC.md
/// §3.1. Section headers are NOT selection stops in the Browser (that's a
/// Dashboard-only, S6 concept) — they're rendered, just not part of
/// `state.visible`.
///
/// Contract asserted by `petri/tests/s5_snapshot.rs` (structural, same
/// approach as S4's `app::render` — see that file's module doc comment for
/// why):
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
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }

    // Layout: header (row 0) | heavy rule (row 1) | main | footer (last row).
    // The rule matches the Dashboard's header chrome (dashboard.rs's
    // header_lines) so Tab between the two screens doesn't feel like a jump
    // to a differently-styled app.
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    // Header: a badged "petri · browser" title — same inverted-color badge
    // treatment as the Dashboard's title, not just colored text.
    let header = Paragraph::new(Line::from(Span::styled(
        " petri · browser ",
        Style::default().fg(Color::Black).bg(theme::ACCENT).add_modifier(Modifier::BOLD),
    )))
    .wrap(Wrap { trim: false });
    frame.render_widget(header, chunks[0]);

    let rule = Paragraph::new(Line::from(Span::styled(
        "═".repeat(area.width as usize),
        Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(rule, chunks[1]);

    // Footer: bound keymap (spec §5 — advertise only keys actually bound).
    let footer = Paragraph::new(Line::from(Span::styled(
        // SPEC.md §3.1: advertise only keys actually bound. The three action
        // keys are bound unconditionally, and deliberately so — `g` always
        // resolves thanks to ACT-3's git fallback, and `o`/`e` always respond,
        // with a notice when this machine or this project can't satisfy them.
        // Making the footer itself machine-dependent was considered and
        // rejected: it would make every snapshot test depend on what happens to
        // be installed on the machine running it.
        " Tab Dashboard  j/k up-down  Shift+j/k ×10  PgUp/PgDn  / filter  o remote  g git log  e edit  q quit ",
        Style::default().fg(theme::DIM),
    )))
    .wrap(Wrap { trim: false });
    frame.render_widget(footer, chunks[3]);

    // Main area: split into list (left) + detail + scrollbar (right), if wide
    // enough for both. Below `DETAIL_PANE_THRESHOLD` the detail pane is hidden
    // entirely to avoid squeezing it into an unreadable sliver.
    let (list_area, detail_inner): (Rect, Option<Rect>) = {
        if area.width >= DETAIL_PANE_THRESHOLD {
            let main = chunks[2];
            let list_width = (main.width * 2) / 3;
            let detail_and_scrollbar_w = main.width - list_width;
            let detail_width = detail_and_scrollbar_w.saturating_sub(1);
            if detail_width >= DETAIL_PANE_DETAIL_MIN {
                let hsplit = Layout::horizontal([
                    Constraint::Length(list_width),
                    Constraint::Length(detail_and_scrollbar_w),
                ])
                .split(main);
                (hsplit[0], Some(hsplit[1]))
            } else {
                (main, None)
            }
        } else {
            (chunks[2], None)
        }
    };

    // List content: section headers + rows. Section headers are rendered as
    // styled lines interleaved with project rows but — because the header
    // lines don't occupy `state.visible` positions and we never highlight a
    // header — they are not selection stops. `selected_line` is the actual
    // line index of the selected row within `list_lines` (accounting for the
    // interleaved headers/blank separators) — NOT the same as `state.selected`,
    // which is an index into `state.visible` (project rows only). Feeding
    // `state.selected` straight into the scroll math was a real bug: the
    // header line would scroll out of view the moment the selection moved at
    // all, and the view could never scroll far enough to reach the tail of a
    // list with several sections above it, because the (smaller)
    // project-space index was always less than the row's true line position.
    // `list_area` is the OUTER rect handed to the bordered Paragraph below —
    // its top+bottom border rows aren't content rows. Computing `visible_rows`
    // from the outer height (rather than the inner, post-border height) was a
    // real bug: the scroll math believed 2 more rows were visible than
    // actually were, so the selection could move 2 rows past the true bottom
    // of the visible list — and the detail pane, which always reflects
    // `state.selected` regardless of scroll, kept showing those still
    // off-screen rows' details — before `compute_scroll_offset` finally
    // caught up and scrolled the list.
    let (list_lines, selected_line) = render_list_lines(radar, state);
    let visible_rows = list_content_rows(list_area.height); // top + bottom border consumed
    let scroll_offset = compute_scroll_offset(selected_line, list_lines.len(), visible_rows);

    let list_para = Paragraph::new(list_lines)
        .block(Block::default().title(" Projects ").borders(Borders::ALL))
        .wrap(Wrap { trim: false })
        .scroll((scroll_offset as u16, 0));
    frame.render_widget(list_para, list_area);

    // Scrollbar: only when we also render a detail pane (so it serves the
    // list that's actually scrolling).
    if let Some(detail_area) = &detail_inner {
        let scrollbar_area = Rect {
            x: detail_area.x + detail_area.width - 1,
            y: detail_area.y,
            width: 1,
            height: detail_area.height,
        };
        let mut sb_state = ScrollbarState::new(state.visible.len())
            .position(scroll_offset);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut sb_state);
    }

    // Detail pane: only when the window is wide enough.
    if let Some(detail_area) = detail_inner {
        render_detail_pane(frame, detail_area, radar, state);
    }
}

/// Number of project-list content rows actually visible within a bordered
/// list block whose OUTER height (the `Rect` handed to it, borders included)
/// is `block_height`. `Borders::ALL` consumes 2 of those rows (top + bottom),
/// so this is `block_height - 2`, floored at 1 (never claim zero visible rows
/// — `compute_scroll_offset` divides conceptually by this and a real 0 would
/// make every offset "already visible").
fn list_content_rows(block_height: u16) -> usize {
    let inner = block_height.saturating_sub(2);
    if inner > 0 { inner as usize } else { 1 }
}

/// Number of list rows a `PageUp`/`PageDown` press should jump by, given the
/// full terminal height (not the list block's height — callers outside this
/// module, i.e. `lib.rs`'s key handler, only have `crossterm::terminal::size()`
/// to work with). Mirrors `render`'s own layout exactly: 3 rows of screen
/// chrome (header + heavy rule + footer, `render`'s `chunks`) surround the
/// main area, whose full height becomes the list block's OUTER height (the
/// list/detail split is horizontal only), which `list_content_rows` then
/// reduces by the list block's own border rows.
pub fn page_size(terminal_height: u16) -> usize {
    list_content_rows(terminal_height.saturating_sub(3))
}

/// Compute the scroll offset so that the selected row stays visible in a list
/// whose visible height is `visible_rows` lines. Clamps to the maximum
/// scrollable offset (list length — visible height). Returns 0 when the list
/// fits entirely in the area.
///
/// Minimal scroll, not eager centering: the offset stays 0 for as long as
/// the selection fits in the first `visible_rows` lines, and only grows once
/// the selection would otherwise fall below the bottom of the view — just
/// enough to keep it at the bottom edge. A real bug, found via human
/// smoke-testing: the previous "target the middle of the window" formula
/// scrolled by a line the moment the cursor moved at all (even far from
/// either edge), which visibly scrolled the first section's header out of
/// view on literally the very first `j` press.
fn compute_scroll_offset(
    selected_line: Option<usize>,
    list_len: usize,
    visible_rows: usize,
) -> usize {
    if list_len == 0 || visible_rows == 0 || visible_rows >= list_len {
        return 0;
    }
    let selected_line = match selected_line {
        Some(l) => l,
        None => return 0,
    };
    let max_scroll = list_len - visible_rows;
    selected_line.saturating_sub(visible_rows - 1).min(max_scroll)
}

/// Build the list's lines: section headers interleaved with project rows, in
/// SECTION_ORDER. Sections with 0 visible projects are skipped entirely.
/// Returns the lines plus the LINE index of the selected row (`None` if
/// nothing is selected) — the caller needs the real line position, not
/// `state.selected` (which only counts project rows, not the headers/blank
/// separators interleaved between them), to compute a correct scroll offset.
fn render_list_lines(radar: &Radar, state: &BrowserState) -> (Vec<Line<'static>>, Option<usize>) {
    let mut lines = Vec::new();
    let mut selected_line: Option<usize> = None;

    if state.visible.is_empty() {
        // "Nothing selected" state per petri/SPEC.md §3.1 — render a gentle
        // placeholder rather than an empty frame. Must not panic.
        lines.push(Line::from(Span::styled(
            "  (no projects)",
            Style::default().fg(theme::DIM),
        )));
        return (lines, None);
    }

    let mut idx_in_visible = 0usize;

    // `is_last_populated_section` tracks whether any later section in the
    // iteration still has visible rows. We compute it by scanning ahead.
    let section_is_last = |target: &StatusBucket| -> bool {
        SECTION_ORDER
            .iter()
            .skip_while(|&s| *s != *target)
            .any(|&s| {
                radar.projects.iter().any(|p| p.status_bucket == s && !p.is_foreign)
                    && state.visible.iter().any(|&idx| {
                        idx < radar.projects.len()
                            && radar.projects[idx].status_bucket == s
                    })
            })
    };

    for (section, label) in &SECTION_LABELS {
        let section_indices: Vec<usize> = state
            .visible
            .iter()
            .copied()
            .filter(|&idx| radar.projects[idx].status_bucket == *section)
            .collect();

        if section_indices.is_empty() {
            continue;
        }

        // Header line: "RUNNING [5]", colored by `theme::bucket_color` — same
        // silence-gradient-as-label-color convention as the Dashboard's
        // section headers (dashboard.rs's `section_header_line`), so the two
        // screens' headers read as one vocabulary rather than the Dashboard's
        // gradient and a flat Browser yellow that happened to coexist.
        lines.push(Line::from(Span::styled(
            format!(" {} [{}] ", label, section_indices.len()),
            Style::default()
                .fg(theme::bucket_color(*section))
                .add_modifier(Modifier::BOLD),
        )));

        for proj_idx in section_indices {
            let is_selected = state.selected.map(|p| p == idx_in_visible).unwrap_or(false);
            if is_selected {
                selected_line = Some(lines.len());
            }
            lines.push(render_project_row(radar, proj_idx, is_selected));
            idx_in_visible += 1;
        }

        // Blank separator between non-last populated sections.
        if !section_is_last(section) {
            lines.push(Line::from(""));
        }
    }

    (lines, selected_line)
}

/// Render one project row: glyph (● working / ○ otherwise), name, dirty marker
/// + uncommitted count, silence age. Selection highlight applied via `is_selected`.
fn render_project_row(radar: &Radar, proj_idx: usize, is_selected: bool) -> Line<'static> {
    let project = &radar.projects[proj_idx];

    let glyph = match project.agent.state {
        AgentActivity::Working => "●",
        _ => "○",
    };

    let name = &project.name;
    let dirty_marker = present::dirty_marker(&project.git);

    let uncommitted = if project.git.uncommitted_files > 0 {
        format!("✎{}", project.git.uncommitted_files)
    } else {
        String::from(" ")
    };

    let silence = silence_display(project.last_activity_at);

    // Selection = reverse video (black on accent), bold — the same
    // convention `dashboard.rs`'s `solid_selected_line` uses for its compact
    // rows: "reverse video is the canonical current-selection signal"
    // (`references/visual-patterns.md`), applied as a background fill rather
    // than a text-color shift so it reads as a bar, not just a tint — the
    // same reason `not-selected` uses `FG` (near-white) rather than a color
    // close enough to `ACCENT` to blur the two states together.
    let style = if is_selected {
        Style::default().fg(Color::Black).bg(theme::ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::FG)
    };
    let meta_style = if is_selected {
        style
    } else {
        Style::default().fg(theme::DIM)
    };

    Line::from(vec![
        Span::styled(format!(" {} ", glyph), style),
        Span::styled(format!("{}{}", name, dirty_marker), style),
        Span::styled(format!("  {}", uncommitted), meta_style),
        Span::styled(format!(" {}", silence), meta_style),
    ])
}

/// Format a `last_activity_at` timestamp as "Xm ago" / "Xh ago" / "Xd ago" /
/// "just now", or "no activity" when absent. Uses the spec's hint of computing
/// a rough humanised duration — exact output is not pinned by tests.
fn silence_display(last_activity_at: Option<chrono::DateTime<chrono::Utc>>) -> String {
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let ts = match last_activity_at {
        Some(dt) => dt.timestamp(),
        None => return "no activity".to_string(),
    };

    let delta = now_secs - ts;
    if delta < 0 {
        return "now".to_string();
    }
    let secs = delta as u64;
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// Detail pane: path (abbreviated), branch, dirty count, commit times, github
/// url, agent state + active agent, session id, last activity. Rendered only
/// when the detail area is wide enough (caller enforces `DETAIL_PANE_DETAIL_MIN`).
fn render_detail_pane(
    frame: &mut Frame,
    area: Rect,
    radar: &Radar,
    state: &BrowserState,
) {
    let (title, body_lines): (&str, Vec<Line<'static>>) = match state.selected_project(radar) {
        Some(project) => (" Detail ", render_detail_lines(project)),
        None => (
            "",
            vec![Line::from(Span::styled(
                "  No project selected",
                Style::default().fg(theme::DIM),
            ))],
        ),
    };

    let detail_block = Block::default().title(title).borders(Borders::ALL);
    let para = Paragraph::new(body_lines).block(detail_block).wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

/// Lines for the detail pane of one project. Each line is a label + value,
/// left-padded so the values align vertically.
fn render_detail_lines(project: &Project) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Path (abbreviated with `~`).
    let display_path = abbreviate_home(&project.path);
    lines.push(Line::from(Span::styled(
        format!("  Path: {}", display_path),
        Style::default().fg(theme::FG),
    )));

    // Branch.
    let branch = project.git.branch.as_deref().unwrap_or("(none)");
    lines.push(Line::from(Span::styled(
        format!("  Branch: {}", branch),
        Style::default().fg(theme::BRANCH),
    )));

    // Dirty / uncommitted count.
    let dirty_line = if project.git.is_dirty {
        Line::from(Span::styled(
            format!(
                "  Dirty: {} uncommitted",
                project.git.uncommitted_files
            ),
            Style::default().fg(theme::DANGER),
        ))
    } else {
        Line::from(Span::styled(
            "  Dirty: clean",
            Style::default().fg(theme::FRESH),
        ))
    };
    lines.push(dirty_line);

    // Last commit time (plus `mine_last_commit_at` when it differs).
    match (&project.git.last_commit_at, &project.git.mine_last_commit_at) {
        (Some(last), Some(mines)) if last != mines => {
            lines.push(Line::from(Span::styled(
                format!("  Last commit: {}", format_commit(*last)),
                Style::default().fg(theme::FG),
            )));
            lines.push(Line::from(Span::styled(
                format!("  Mine      : {}", format_commit(*mines)),
                Style::default().fg(theme::FRESH),
            )));
        }
        (Some(last), _) => {
            lines.push(Line::from(Span::styled(
                format!("  Last commit: {}", format_commit(*last)),
                Style::default().fg(theme::FG),
            )));
        }
        (None, _) => {
            lines.push(Line::from(Span::styled(
                "  Last commit: (none)",
                Style::default().fg(theme::DIM),
            )));
        }
    }

    // GitHub URL — same accent as the Dashboard's `[gh]` marker
    // (dashboard.rs's `compact_row_line`), so the same fact reads as the
    // same color on both screens.
    if let Some(url) = &project.git.github_url {
        lines.push(Line::from(Span::styled(
            format!("  GitHub: {}", url),
            Style::default().fg(theme::ACCENT),
        )));
    }

    // Agent state + active agent.
    let agent_label = present::agent_label(&project.agent);
    lines.push(Line::from(Span::styled(
        format!("  Agent: {}", agent_label),
        Style::default().fg(if project.agent.state == AgentActivity::Working {
            theme::FRESH
        } else {
            theme::FG
        }),
    )));

    // Session id.
    if let Some(session_id) = &project.agent.session_id {
        lines.push(Line::from(Span::styled(
            format!("  Session: {}", session_id),
            Style::default().fg(theme::DIM),
        )));
    }

    // Last activity.
    let last_activity = match project.last_activity_at {
        Some(dt) => format_commit(dt),
        None => "no activity".to_string(),
    };
    lines.push(Line::from(Span::styled(
        format!("  Last activity: {}", last_activity),
        Style::default().fg(theme::DIM),
    )));

    lines
}

/// Format an RFC-3339 timestamp as "YYYY-MM-DD HH:MM" for the detail pane.
fn format_commit(dt: chrono::DateTime<chrono::Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M").to_string()
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
            let start = if remainder.starts_with('/') { "" } else { "/" };
            return format!("~{}{}", start, remainder);
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use petridish_core::schema::{AgentState, GitState};

    fn project(id: &str, name: &str, bucket: StatusBucket) -> Project {
        Project {
            id: id.to_string(),
            name: name.to_string(),
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

    fn radar_of(projects: Vec<Project>) -> Radar {
        Radar {
            schema_version: 1,
            updated_at: chrono::Utc::now(),
            scan_duration_ms: 0,
            projects,
            quota: None,
        }
    }

    /// Regression test for a real scroll bug found via human smoke-testing:
    /// `render_list_lines`'s `selected_line` must be the row's actual LINE
    /// index (accounting for the section header pushed above it), not
    /// `state.selected`, which only counts project rows. Feeding the wrong
    /// index space into `compute_scroll_offset` made the header scroll out
    /// of view the moment the cursor moved even once, and made the list
    /// unable to scroll far enough to reach items near the bottom of a list
    /// with several sections stacked above them.
    #[test]
    fn selected_line_accounts_for_the_section_header_above_it() {
        let radar = radar_of(vec![
            project("a", "alpha", StatusBucket::Active),
            project("b", "beta", StatusBucket::Active),
            project("c", "gamma", StatusBucket::Active),
        ]);
        let mut state = BrowserState::new(&radar);

        // Line 0 is the "RUNNING [3]" header, so the first project row (the
        // default selection) must be line 1, not line 0.
        let (_, selected_line) = render_list_lines(&radar, &state);
        assert_eq!(selected_line, Some(1), "the header line must be accounted for");

        state.move_selection(1);
        let (_, selected_line) = render_list_lines(&radar, &state);
        assert_eq!(selected_line, Some(2), "moving selection by one project row must move the line index by one, not reset relative to the header");

        state.move_selection(1);
        let (_, selected_line) = render_list_lines(&radar, &state);
        assert_eq!(selected_line, Some(3));
    }

    /// A second, more direct regression check: with enough sections stacked
    /// above it that the true line index diverges further from the
    /// project-space index, the scroll offset computed from the correct line
    /// index must be able to reach the tail of the list — the bug this
    /// guards against silently capped the reachable offset far short of the
    /// list's actual end whenever headers preceded the selection.
    #[test]
    fn scroll_offset_can_reach_the_tail_of_a_multi_section_list() {
        let mut projects = Vec::new();
        for i in 0..3 {
            projects.push(project(&format!("r{i}"), &format!("running-{i}"), StatusBucket::Active));
        }
        for i in 0..10 {
            projects.push(project(&format!("f{i}"), &format!("flight-{i}"), StatusBucket::InFlight));
        }
        let radar = radar_of(projects);
        let mut state = BrowserState::new(&radar);

        // Move to the very last project row.
        for _ in 0..20 {
            state.move_selection(1);
        }

        let (list_lines, selected_line) = render_list_lines(&radar, &state);
        let visible_rows = 5usize;
        let scroll_offset = compute_scroll_offset(selected_line, list_lines.len(), visible_rows);

        // The last project row must be within the visible window: its line
        // index must be less than `scroll_offset + visible_rows`.
        let selected_line = selected_line.expect("a project must be selected");
        assert!(
            selected_line < scroll_offset + visible_rows,
            "selected line {selected_line} must be within the visible window [{scroll_offset}, {})",
            scroll_offset + visible_rows
        );
    }

    /// Regression test for a real scroll bug found via human smoke-testing:
    /// `render`'s `visible_rows` was computed from the list block's OUTER
    /// height (the `Rect` handed to the bordered Paragraph), not its inner,
    /// post-border content height. Because `Borders::ALL` consumes 2 rows
    /// (top + bottom), the scroll math believed 2 more rows were on screen
    /// than actually were — so pressing down could move the selection 2 rows
    /// past the true bottom of the visible list, with the (always-correct,
    /// scroll-independent) detail pane still showing details for a row that
    /// had actually scrolled out of view, before the list caught up.
    ///
    /// Exercises the real `render()` entry point end-to-end through a
    /// `TestBackend` buffer (not just `compute_scroll_offset` in isolation)
    /// so it fails the way the original bug actually manifested.
    #[test]
    fn last_selected_row_is_actually_visible_in_the_rendered_buffer() {
        use ratatui::{backend::TestBackend, Terminal};

        // Enough project rows that a small terminal can't show them all at
        // once, so a scroll-lag bug has room to manifest.
        let projects: Vec<Project> = (0..20)
            .map(|i| project(&format!("p{i}"), &format!("project-{i}"), StatusBucket::Active))
            .collect();
        let radar = radar_of(projects);
        let mut state = BrowserState::new(&radar);
        for _ in 0..100 {
            state.move_selection(1); // clamps at the last row
        }

        // Narrower than `DETAIL_PANE_THRESHOLD` so the detail pane (which
        // always shows the selected project's name regardless of scroll) is
        // hidden entirely — the only way "project-19" can appear in the
        // buffer is if the list itself actually scrolled to show it.
        let width = 40u16;
        let height = 15u16; // short enough that not all 20 rows fit
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("TestBackend terminal must construct");
        terminal.draw(|frame| render(frame, &radar, &state)).expect("draw must not error");
        let buffer = terminal.backend().buffer();
        let whole: String = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            whole.contains("project-19"),
            "the selected (last) project's row must actually be visible in the rendered list, not just reflected in the detail pane, got:\n{whole}"
        );
    }

    /// `page_size` backs `PageUp`/`PageDown` (lib.rs) — it must account for
    /// exactly the same chrome `render` does: 3 screen rows (header, heavy
    /// rule, footer) plus the list block's own 2 border rows, 5 total.
    #[test]
    fn page_size_accounts_for_screen_chrome_and_list_borders() {
        // 24-row terminal: 24 - 3 (header/rule/footer) - 2 (list borders) = 19.
        assert_eq!(page_size(24), 19);
        // 80x50: 50 - 5 = 45.
        assert_eq!(page_size(50), 45);
        // Degenerate terminals must floor at 1, never 0 (a 0-row page jump
        // would be a silent no-op) and must never underflow/panic.
        assert_eq!(page_size(3), 1);
        assert_eq!(page_size(0), 1);
    }

    /// Regression test for a second, related real bug found via human
    /// smoke-testing: the scroll offset must stay 0 (no scroll at all) as
    /// long as the selection already fits within the visible window — the
    /// previous "target the middle of the window" formula scrolled by a
    /// line the moment the cursor moved even once, which visibly scrolled
    /// the first section's header out of view on the very first `j` press
    /// despite there being no need to scroll at all yet.
    #[test]
    fn scroll_offset_stays_zero_while_the_selection_already_fits_in_view() {
        let mut projects = Vec::new();
        for i in 0..3 {
            projects.push(project(&format!("r{i}"), &format!("running-{i}"), StatusBucket::Active));
        }
        for i in 0..10 {
            projects.push(project(&format!("f{i}"), &format!("flight-{i}"), StatusBucket::InFlight));
        }
        let radar = radar_of(projects);
        let mut state = BrowserState::new(&radar);
        let visible_rows = 14usize; // comfortably fits the header + first few rows

        // Move down twice — well within the visible window — and confirm no
        // scrolling happened at all.
        for _ in 0..2 {
            state.move_selection(1);
            let (list_lines, selected_line) = render_list_lines(&radar, &state);
            let scroll_offset = compute_scroll_offset(selected_line, list_lines.len(), visible_rows);
            assert_eq!(
                scroll_offset, 0,
                "selection at line {selected_line:?} still fits within {visible_rows} visible rows — must not scroll yet"
            );
        }
    }
}

/// A one-line transient message over the Browser — "nothing installed that can
/// open in editor", "thing has no remote" (`ACT-9`).
///
/// Deliberately a thin bar rather than a dialog: it is informational, needs no
/// answer, and is dismissed by the next keystroke. A modal would demand an
/// acknowledgement the user has no decision to make about.
pub fn render_notice(frame: &mut Frame, text: &str) {
    use ratatui::widgets::Clear;

    let area = frame.area();
    if area.height < 3 {
        return;
    }
    let width = (text.chars().count() as u16 + 4).min(area.width);
    let bar = Rect {
        x: area.width.saturating_sub(width) / 2,
        y: area.height.saturating_sub(2),
        width,
        height: 1,
    };
    frame.render_widget(Clear, bar);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  {text}  "),
            Style::default()
                .fg(Color::Black)
                .bg(crate::theme::AGING)
                .add_modifier(Modifier::BOLD),
        ))),
        bar,
    );
}
