//! The Browser screen (S5): grouped project list, selection, detail pane,
//! type-ahead filter. petri/SPEC.md §3.1 is the authoritative behavior
//! contract — read it in full before touching this file.

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
    /// empty.
    pub fn move_selection(&mut self, delta: i32) {
        if self.visible.is_empty() {
            return;
        }
        let n = self.visible.len() as i32;
        let current = self.selected.unwrap_or(0) as i32;
        self.selected = Some((current + delta).clamp(0, n - 1) as usize);
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

    // Layout: header (row 0) | main (rows 1..H-1) | footer (last row).
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    // Header: "petri · browser" identifying the app on row 0.
    let header = Paragraph::new(Line::from(Span::styled(
        "petri · browser",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )))
    .wrap(Wrap { trim: false });
    frame.render_widget(header, chunks[0]);

    // Footer: bound keymap (spec §5 — advertise only keys actually bound).
    let footer = Paragraph::new(Line::from(Span::styled(
        " Tab Dashboard  j/k up-down  / filter  q quit ",
        Style::default().fg(Color::DarkGray),
    )))
    .wrap(Wrap { trim: false });
    frame.render_widget(footer, chunks[2]);

    // Main area: split into list (left) + detail + scrollbar (right), if wide
    // enough for both. Below `DETAIL_PANE_THRESHOLD` the detail pane is hidden
    // entirely to avoid squeezing it into an unreadable sliver.
    let (list_area, detail_inner): (Rect, Option<Rect>) = {
        if area.width >= DETAIL_PANE_THRESHOLD {
            let main = chunks[1];
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
            (chunks[1], None)
        }
    };

    // List content: section headers + rows. Section headers are rendered as
    // styled lines interleaved with project rows but — because the header
    // lines don't occupy `state.visible` positions and we never highlight a
    // header — they are not selection stops.
    let list_lines = render_list_lines(radar, state);
    let visible_rows = if list_area.height > 0 { list_area.height as usize } else { 1 };
    let scroll_offset = compute_scroll_offset(state.selected, list_lines.len(), visible_rows);

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

/// Compute the scroll offset so that the selected row stays visible in a list
/// whose visible height is `visible_rows` lines. Clamps to the maximum
/// scrollable offset (list length — visible height). Returns 0 when the list
/// fits entirely in the area.
fn compute_scroll_offset(
    selected: Option<usize>,
    list_len: usize,
    visible_rows: usize,
) -> usize {
    if list_len == 0 || visible_rows <= 0 {
        return 0;
    }
    if visible_rows >= list_len {
        return 0;
    }
    let selected = match selected {
        Some(p) => p,
        None => return 0,
    };
    let max_scroll = list_len - visible_rows;
    if max_scroll <= 0 {
        return 0;
    }
    // Clamp the selected line to the visible window: its top-of-view offset
    // must be at most `selected` (so it's not above the top of view) and at
    // least `selected - (visible_rows - 1)` (so it's not below the bottom).
    let upper = selected; // can scroll so selection is at the top of view
    let lower = selected.saturating_sub(visible_rows - 1); // so it's at the bottom
    let upper = upper.min(max_scroll);
    let lower = lower.min(max_scroll);
    if upper < lower { return 0; }
    // Target: selection roughly in the middle of the visible window.
    (lower + upper) / 2
}

/// Build the list's lines: section headers interleaved with project rows, in
/// SECTION_ORDER. Sections with 0 visible projects are skipped entirely.
fn render_list_lines(radar: &Radar, state: &BrowserState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if state.visible.is_empty() {
        // "Nothing selected" state per petri/SPEC.md §3.1 — render a gentle
        // placeholder rather than an empty frame. Must not panic.
        lines.push(Line::from(Span::styled(
            "  (no projects)",
            Style::default().fg(Color::DarkGray),
        )));
        return lines;
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

        // Header line: "RUNNING [5]" in yellow+bold.
        lines.push(Line::from(Span::styled(
            format!(" {} [{}] ", label, section_indices.len()),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));

        for proj_idx in section_indices {
            let is_selected = state.selected.map(|p| p == idx_in_visible).unwrap_or(false);
            lines.push(render_project_row(radar, proj_idx, is_selected));
            idx_in_visible += 1;
        }

        // Blank separator between non-last populated sections.
        if !section_is_last(section) {
            lines.push(Line::from(""));
        }
    }

    lines
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

    let style = if is_selected {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };

    Line::from(vec![
        Span::styled(format!(" {} ", glyph), style),
        Span::styled(format!("{}{}", name, dirty_marker), style),
        Span::styled(format!("  {}", uncommitted), Style::default().fg(Color::DarkGray)),
        Span::styled(format!(" {}", silence), Style::default().fg(Color::DarkGray)),
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
                Style::default().fg(Color::DarkGray),
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
        Style::default().fg(Color::White),
    )));

    // Branch.
    let branch = project.git.branch.as_deref().unwrap_or("(none)");
    lines.push(Line::from(Span::styled(
        format!("  Branch: {}", branch),
        Style::default().fg(Color::Cyan),
    )));

    // Dirty / uncommitted count.
    let dirty_line = if project.git.is_dirty {
        Line::from(Span::styled(
            format!(
                "  Dirty: {} uncommitted",
                project.git.uncommitted_files
            ),
            Style::default().fg(Color::Red),
        ))
    } else {
        Line::from(Span::styled(
            "  Dirty: clean",
            Style::default().fg(Color::Green),
        ))
    };
    lines.push(dirty_line);

    // Last commit time (plus `mine_last_commit_at` when it differs).
    match (&project.git.last_commit_at, &project.git.mine_last_commit_at) {
        (Some(last), Some(mines)) if last != mines => {
            lines.push(Line::from(Span::styled(
                format!("  Last commit: {}", format_commit(*last)),
                Style::default().fg(Color::White),
            )));
            lines.push(Line::from(Span::styled(
                format!("  Mine      : {}", format_commit(*mines)),
                Style::default().fg(Color::Green),
            )));
        }
        (Some(last), _) => {
            lines.push(Line::from(Span::styled(
                format!("  Last commit: {}", format_commit(*last)),
                Style::default().fg(Color::White),
            )));
        }
        (None, _) => {
            lines.push(Line::from(Span::styled(
                "  Last commit: (none)",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    // GitHub URL.
    if let Some(url) = &project.git.github_url {
        lines.push(Line::from(Span::styled(
            format!("  GitHub: {}", url),
            Style::default().fg(Color::Green),
        )));
    }

    // Agent state + active agent.
    let agent_label = present::agent_label(&project.agent);
    lines.push(Line::from(Span::styled(
        format!("  Agent: {}", agent_label),
        Style::default().fg(if project.agent.state == AgentActivity::Working {
            Color::Yellow
        } else {
            Color::White
        }),
    )));

    // Session id.
    if let Some(session_id) = &project.agent.session_id {
        lines.push(Line::from(Span::styled(
            format!("  Session: {}", session_id),
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Last activity.
    let last_activity = match project.last_activity_at {
        Some(dt) => format_commit(dt),
        None => "no activity".to_string(),
    };
    lines.push(Line::from(Span::styled(
        format!("  Last activity: {}", last_activity),
        Style::default().fg(Color::DarkGray),
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
