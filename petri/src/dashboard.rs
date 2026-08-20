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
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

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

impl DashboardState {
    /// Membership for the `RUNNING` (Active) section per ADR-0001: a project
    /// counts as running if its own `status_bucket` is `Active`, OR it has at
    /// least one worktree child (`parent_path == Some(this project's path)`)
    /// whose own `status_bucket` is `Active`. `is_foreign` projects are
    /// always excluded. Display-only — never mutates `status_bucket`.
    ///
    /// Ordered quietest first: oldest `last_activity_at` first, `None`
    /// treated as maximally silent (sorts before any `Some`).
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
            let a_activity = radar.projects[a].last_activity_at;
            let b_activity = radar.projects[b].last_activity_at;
            match (a_activity, b_activity) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(at_a), Some(at_b)) => at_a.cmp(&at_b),
            }
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

/// Render the Dashboard into `frame`. Per petri/SPEC.md §3.2:
/// - Header: `petri · dashboard`, project count, clock, last scan duration.
/// - `RUNNING` section: roomy cards (glyph, name, `silent Xm · agent-name`,
///   branch, dirty marker, `commit 1d ago`, `~`-abbreviated path, truncated
///   session id).
/// - `IN FLIGHT`/`STALE`/`COLD`: compact rows (name, branch, `✎N`, commit
///   age, `gh` marker).
/// - Collapsed sections still render their header + count, but no rows.
/// - **Overflow: truncate, never scroll.** If expanded sections exceed the
///   available height, sections emit in priority order (`SECTION_ORDER`) and
///   stop, with a required `… +N more` marker at the cut.
/// - Staleness banner when `radar.updated_at` is older than 24h.
/// - Must not panic on an empty `radar.projects`, nor at 0×0 or 1×1.
///
/// Worktree nesting/rollup (indented children, `name · N worktrees` rollup
/// counts in compact sections) is deliberately NOT attempted here — the
/// acceptance gate (`s6_dashboard.rs`'s module doc comment) documents this as
/// ambiguous against the only fixture that exercises it, and does not assert
/// it. Left as a follow-up once the spec's ambiguity for a parent whose own
/// section differs from its worktree child's section is resolved.
pub fn render(frame: &mut ratatui::Frame, radar: &Radar, state: &DashboardState) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    // Bail at 1 row or shorter: we need at least a header line.
    if area.height <= 1 {
        return;
    }

    let elapsed_secs = chrono::Utc::now().signed_duration_since(radar.updated_at).num_seconds().max(0);
    let is_stale = elapsed_secs > 86400;

    let now = chrono::Utc::now();
    let scan_secs = radar.scan_duration_ms as f64 / 1000.0;

    let stale_banner: Option<Line<'static>> = if is_stale {
        Some(Line::from(Span::styled(
            format!(" ⚠ Data stale (updated {} ago)", humanize_secs(elapsed_secs as u64)),
            Style::default().fg(Color::Red),
        )))
    } else {
        None
    };

    let available_content = (area.height as usize)
        .saturating_sub(2) // header + footer
        .saturating_sub(if is_stale { 1 } else { 0 });

    // Build content lines: section headers + project rows, truncating with a
    // required "+N more" marker the moment we'd exceed available_content.
    let mut content_lines: Vec<Line<'static>> = Vec::new();
    'sections: for (si, bucket) in SECTION_ORDER.iter().enumerate() {
        let members = DashboardState::section_members(radar, *bucket);
        if members.is_empty() {
            continue;
        }

        if content_lines.len() >= available_content {
            break;
        }
        let is_selected_header = matches!(
            state.selected.and_then(|i| state.visible.get(i)),
            Some(DashRow::Header(b)) if *b == *bucket
        );
        content_lines.push(section_header_line(radar, *bucket, members.len(), is_selected_header));

        if !state.collapsed[si] {
            for (member_pos, &proj_idx) in members.iter().enumerate() {
                if content_lines.len() >= available_content {
                    let remaining = members.len() - member_pos;
                    content_lines.push(Line::from(Span::styled(
                        format!(" … +{remaining} more"),
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    )));
                    continue 'sections;
                }
                let is_selected_row = matches!(
                    state.selected.and_then(|i| state.visible.get(i)),
                    Some(DashRow::Project(idx)) if *idx == proj_idx
                );
                content_lines.push(if *bucket == StatusBucket::Active {
                    roomy_card_line(radar, proj_idx, is_selected_row)
                } else {
                    compact_row_line(radar, proj_idx, is_selected_row)
                });
            }
        }
    }

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(2 + content_lines.len() + usize::from(is_stale));
    lines.push(header_line(radar, &now, scan_secs));
    if let Some(banner) = stale_banner {
        lines.push(banner);
    }
    lines.extend(content_lines);
    lines.push(footer_line());

    // Deliberately NOT using `Wrap` here: each entry in `content_lines` is
    // counted as exactly one physical row against `available_content` above
    // — if a long roomy card line were allowed to wrap to two physical rows,
    // the "+N more" truncation marker could be pushed off the bottom of the
    // terminal (clipped by the widget boundary) rather than actually shown,
    // which is exactly the "silent truncation" failure mode the spec calls
    // out as unacceptable. No wrapping means an overlong line is clipped at
    // the right edge instead — visually lossy for that one row, but the
    // truncation marker itself stays guaranteed-visible.
    let para = Paragraph::new(lines);
    frame.render_widget(para, area);
}

/// Header line: `petri · dashboard`, project count, clock, scan duration.
fn header_line(radar: &Radar, now: &chrono::DateTime<chrono::Utc>, scan_secs: f64) -> Line<'static> {
    Line::from(vec![
        Span::styled("petri · dashboard", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("  | "),
        Span::styled(format!("{} projects", radar.projects.len()), Style::default().fg(Color::White)),
        Span::raw("  | "),
        Span::styled(now.format("%H:%M").to_string(), Style::default().fg(Color::DarkGray)),
        Span::raw("  | "),
        Span::styled(format!("scan {scan_secs:.1}s"), Style::default().fg(Color::DarkGray)),
    ])
}

/// Section header line: "RUNNING [25]". RUNNING degrades to RECENT when no
/// member project has an active agent (`agent.active_agent.is_some()`) —
/// petri/SPEC.md §3.2's "because RUNNING would then overstate it", documented
/// interpretation per `s6_snapshot.rs`'s module doc comment.
fn section_header_line(radar: &Radar, bucket: StatusBucket, count: usize, is_selected: bool) -> Line<'static> {
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
    let style = if is_selected {
        Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    };
    Line::from(Span::styled(format!(" {label} [{count}] "), style))
}

/// Roomy card for a project in the RUNNING section.
fn roomy_card_line(radar: &Radar, proj_idx: usize, is_selected: bool) -> Line<'static> {
    let p = &radar.projects[proj_idx];
    let glyph = match p.agent.state {
        AgentActivity::Working => "●",
        _ => "○",
    };
    let dirty_marker = present::dirty_marker(&p.git);
    let uncommitted = if p.git.uncommitted_files > 0 { format!("✎{}", p.git.uncommitted_files) } else { String::new() };
    let branch = p.git.branch.as_deref().unwrap_or("(none)");
    let commit_age = match p.git.last_commit_at {
        Some(dt) => commit_ago(dt),
        None => "(none)".to_string(),
    };
    let display_path = abbreviate_home(&p.path);
    let session = p.agent.session_id.as_deref().unwrap_or("");
    let silence_secs = match p.last_activity_at {
        Some(dt) => chrono::Utc::now().signed_duration_since(dt).num_seconds().max(0),
        None => 0,
    };
    let agent_name = p.agent.active_agent.as_deref().unwrap_or("idle");

    let style = if is_selected { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::White) };

    Line::from(vec![
        Span::styled(format!(" {glyph} "), style),
        Span::styled(format!("{}{}{} ", p.name, dirty_marker, uncommitted), style),
        Span::styled(format!("silent {} · {} ", humanize_secs(silence_secs as u64), agent_name), Style::default().fg(Color::Cyan)),
        Span::styled(format!("{branch} "), Style::default().fg(Color::Cyan)),
        Span::styled(format!("commit {commit_age} "), Style::default().fg(Color::DarkGray)),
        Span::raw(format!("— {display_path} ")),
        Span::styled(session.to_string(), Style::default().fg(Color::DarkGray)),
    ])
}

/// Compact row for a project in IN FLIGHT / STALE / COLD.
fn compact_row_line(radar: &Radar, proj_idx: usize, is_selected: bool) -> Line<'static> {
    let p = &radar.projects[proj_idx];
    let branch = p.git.branch.as_deref().unwrap_or("(none)");
    let uncommitted = if p.git.uncommitted_files > 0 { format!("✎{}", p.git.uncommitted_files) } else { String::new() };
    let commit_age = match p.git.last_commit_at {
        Some(dt) => commit_ago(dt),
        None => "(none)".to_string(),
    };
    let gh = if p.git.github_url.is_some() { "[gh]" } else { "" };

    let style = if is_selected { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::White) };

    Line::from(vec![
        Span::styled(format!(" {} ", p.name), style),
        Span::styled(format!("{branch} "), Style::default().fg(Color::Cyan)),
        Span::styled(format!("{uncommitted} "), Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{commit_age} "), Style::default().fg(Color::DarkGray)),
        Span::styled(gh, Style::default().fg(Color::Green)),
    ])
}

/// Footer: the keymap advertising only keys actually bound (petri/SPEC.md §5).
fn footer_line() -> Line<'static> {
    Line::from(Span::styled(
        " j/k move  Space toggle  Enter open/browser  Tab Browser  q quit ",
        Style::default().fg(Color::DarkGray),
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
