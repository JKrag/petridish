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

/// Below this many *content* rows (post chrome, see `render`'s
/// `available_content`), RUNNING drops from roomy 4-line cards to the same
/// single-line compact row IN FLIGHT/STALE/COLD already use. This is the
/// "corner iTerm split" case worked out in the dashboard redesign discussion:
/// a real 21-row split is wide enough for a roomy card's fields but too
/// short to show more than a handful of them, so density is driven by row
/// budget, not column width — width is spent widening the one line instead.
const COMPACT_TIER_MAX_CONTENT_ROWS: usize = 16;

/// The dashboard's truecolor identity (superseding SPEC.md §4's ANSI-16
/// mandate, per the redesign discussion — that constraint was written for
/// the unstarted curses-era plan and isn't binding now that ratatui is in
/// real use). One palette shared by chrome and data, not two that happen to
/// coexist: `COLOR_ACCENT` is the app's own color (badge, selection,
/// dividers); `COLOR_FRESH`/`COLOR_AGING`/`COLOR_COLD` are the same
/// green→amber→grey silence gradient `silence_tier_color` uses, reused here
/// so a section's *label* color previews the state of what's inside it
/// (RUNNING reads green-ish, STALE/COLD read grey) instead of every section
/// header being a flat, undifferentiated yellow.
const COLOR_ACCENT: Color = Color::Rgb(0x33, 0xe2, 0xac);
const COLOR_FRESH: Color = Color::Rgb(0x4f, 0xe6, 0xa0);
const COLOR_AGING: Color = Color::Rgb(0xf0, 0xb8, 0x4f);
const COLOR_COLD: Color = Color::Rgb(0x6b, 0x7a, 0x74);
const COLOR_DIMMER: Color = Color::Rgb(0x48, 0x54, 0x4f);
const COLOR_FG: Color = Color::Rgb(0xd9, 0xe6, 0xe0);
const COLOR_DIM: Color = Color::Rgb(0x64, 0x76, 0x6f);
const COLOR_BRANCH: Color = Color::Rgb(0x8f, 0xa3, 0x9b);
const COLOR_DANGER: Color = Color::Rgb(0xef, 0x6a, 0x5b);

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

/// Render the Dashboard into `frame`. Per petri/SPEC.md §3.2, and following
/// petripy's actual chrome (`src/petridish/screens.py`'s `_header`/`_section`)
/// since the first Rust pass under-used the real estate collapsible sections
/// were meant to free up:
/// - Header: a badged `petri · dashboard` title, project count/clock/scan
///   duration on the right, then a HEAVY double rule (`═`) the full width —
///   this is the thing that makes the header read as a header, not "a line
///   of text that nearly disappears into the rest."
/// - Each section is bracketed by light rules (`─`): one above (skipped for
///   the very first section, which already sits under the header's heavy
///   rule) and one below its label line, mirroring petripy's `_section`.
/// - `RUNNING` section: roomy 3-line cards (name/dirty/uncommitted + silence
///   & agent; branch + event-or-commit; `~`-abbreviated path + session id)
///   plus a blank separator line — matching petripy's actual roomy density,
///   not a single crammed line per project.
/// - `IN FLIGHT`/`STALE`/`COLD`: compact single-line rows (name, branch,
///   `✎N`, commit age, `gh` marker).
/// - Collapsed sections still render their header + count, but no rows.
/// - **Overflow: truncate, never scroll.** If expanded sections exceed the
///   available height, sections emit in priority order (`SECTION_ORDER`) and
///   stop, with a required `… +N more` marker at the cut — accounting for
///   each roomy card's real 4-line footprint (3 content + 1 blank), not
///   1 line per item.
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
    let width = area.width as usize;

    let elapsed_secs = chrono::Utc::now().signed_duration_since(radar.updated_at).num_seconds().max(0);
    let is_stale = elapsed_secs > 86400;

    let now = chrono::Utc::now();
    let scan_secs = radar.scan_duration_ms as f64 / 1000.0;

    let stale_banner: Option<Line<'static>> = if is_stale {
        Some(Line::from(Span::styled(
            format!(" ⚠ Data stale (updated {} ago)", humanize_secs(elapsed_secs as u64)),
            Style::default().fg(Color::Black).bg(COLOR_DANGER).add_modifier(Modifier::BOLD),
        )))
    } else {
        None
    };

    // 2 header lines (title + heavy rule) + 2 footer lines (light rule +
    // keymap) + 1 reserved row for a cross-section "not shown" summary (see
    // `skipped_sections` below) — a real 80-project fleet in a 16-row corner
    // split showed this is not a theoretical case: STALE/COLD can fail to
    // fit even their own header, and without this reserved row they vanished
    // with zero indication they existed, which is exactly the "silent
    // truncation" failure mode the spec exists to rule out. Every physical
    // row this function will emit is counted here, so neither marker can be
    // pushed off the bottom of the terminal by a mis-budgeted item.
    let available_content = (area.height as usize)
        .saturating_sub(5)
        .saturating_sub(usize::from(is_stale));

    let compact_tier = available_content <= COMPACT_TIER_MAX_CONTENT_ROWS;

    // Sections that couldn't fit even their own header+count, tracked so a
    // single summary row can name them instead of them silently disappearing.
    let mut skipped_sections: Vec<(StatusBucket, usize)> = Vec::new();

    let mut content_lines: Vec<Line<'static>> = Vec::new();
    'sections: for (si, bucket) in SECTION_ORDER.iter().enumerate() {
        let members = DashboardState::section_members(radar, *bucket);
        if members.is_empty() {
            continue;
        }

        let is_first_section = content_lines.is_empty();
        let chrome_lines = if is_first_section { 2 } else { 3 }; // [rule_above] + label + rule_below
        if content_lines.len() + chrome_lines > available_content {
            // Budget only ever grows tighter from here, so every remaining
            // section (this one included) is unreachable — record all of
            // them in one pass rather than breaking silently.
            skipped_sections.push((*bucket, members.len()));
            for later_bucket in SECTION_ORDER.iter().skip(si + 1) {
                let later_members = DashboardState::section_members(radar, *later_bucket);
                if !later_members.is_empty() {
                    skipped_sections.push((*later_bucket, later_members.len()));
                }
            }
            break;
        }
        if !is_first_section {
            content_lines.push(rule_line(width, Color::DarkGray));
        }
        let is_selected_header = matches!(
            state.selected.and_then(|i| state.visible.get(i)),
            Some(DashRow::Header(b)) if *b == *bucket
        );
        content_lines.push(section_header_line(radar, *bucket, members.len(), is_selected_header, width));
        content_lines.push(rule_line(width, Color::DarkGray));

        if !state.collapsed[si] {
            let roomy_running = *bucket == StatusBucket::Active && !compact_tier;
            // 5 lines per roomy card: 3 content lines + the sparkline + the blank
            // separator (see `roomy_card_lines`) -- must track its actual line count.
            let item_span = if roomy_running { 5 } else { 1 };
            for (member_pos, &proj_idx) in members.iter().enumerate() {
                if content_lines.len() + item_span > available_content {
                    let remaining = members.len() - member_pos;
                    content_lines.push(Line::from(Span::styled(
                        format!(" … +{remaining} more"),
                        Style::default().fg(COLOR_AGING).add_modifier(Modifier::BOLD),
                    )));
                    continue 'sections;
                }
                let is_selected_row = matches!(
                    state.selected.and_then(|i| state.visible.get(i)),
                    Some(DashRow::Project(idx)) if *idx == proj_idx
                );
                if roomy_running {
                    content_lines.extend(roomy_card_lines(radar, proj_idx, is_selected_row, width));
                } else if *bucket == StatusBucket::Active {
                    content_lines.push(compact_running_row_line(radar, proj_idx, is_selected_row));
                } else {
                    content_lines.push(compact_row_line(radar, proj_idx, is_selected_row));
                }
            }
        }
    }

    if !skipped_sections.is_empty() {
        let summary = skipped_sections
            .iter()
            .map(|(bucket, count)| {
                let label = SECTION_LABELS.iter().find(|(b, _)| b == bucket).map(|(_, l)| *l).unwrap_or("?");
                format!("{label} +{count}")
            })
            .collect::<Vec<_>>()
            .join("  ·  ");
        content_lines.push(Line::from(Span::styled(
            format!(" … not shown: {summary} — resize taller"),
            Style::default().fg(COLOR_AGING).add_modifier(Modifier::BOLD),
        )));
    }

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(4 + content_lines.len() + usize::from(is_stale));
    lines.extend(header_lines(radar, &now, scan_secs, width));
    if let Some(banner) = stale_banner {
        lines.push(banner);
    }
    lines.extend(content_lines);
    lines.push(rule_line(width, Color::DarkGray));
    lines.push(footer_line());

    // Deliberately NOT using `Wrap` here: every line pushed above is counted
    // as exactly one physical row against `available_content` — if a long
    // line were allowed to wrap to two physical rows, the "+N more"
    // truncation marker could be pushed off the bottom of the terminal
    // (clipped by the widget boundary) rather than actually shown, which is
    // exactly the "silent truncation" failure mode the spec calls out as
    // unacceptable. No wrapping means an overlong line is clipped at the
    // right edge instead — visually lossy for that one row, but the
    // truncation marker itself stays guaranteed-visible.
    let para = Paragraph::new(lines);
    frame.render_widget(para, area);
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

/// Three-part version of `split_line`: `left`, then a small fixed gap, then `middle`, then
/// whatever space remains before `right`. Exists for roomy cards with real unused width in
/// the middle of line 1 (name/dirty on the left, "silent Xm · agent" on the right) --
/// currently used to slot the git-activity sparkline in there rather than growing the card
/// with another line.
fn split_line3(
    left: String,
    middle: String,
    right: String,
    width: usize,
    left_style: Style,
    middle_style: Style,
    right_style: Style,
) -> Line<'static> {
    let left_len = left.chars().count();
    let middle_len = middle.chars().count();
    let right_len = right.chars().count();

    let gap1 = 2usize.min(width.saturating_sub(left_len));
    let used = left_len + gap1 + middle_len;
    let gap2 = width.saturating_sub(used + right_len).max(1);

    Line::from(vec![
        Span::styled(left, left_style),
        Span::raw(" ".repeat(gap1)),
        Span::styled(middle, middle_style),
        Span::raw(" ".repeat(gap2)),
        Span::styled(right, right_style),
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
        Style::default().fg(Color::Black).bg(COLOR_ACCENT).add_modifier(Modifier::BOLD),
        Style::default().fg(COLOR_FG),
    );
    vec![title, Line::from(Span::styled("═".repeat(width), Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)))]
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
    // fresh-green, IN FLIGHT amber, STALE/COLD grey — one palette, not a
    // flat yellow for every section regardless of what it actually holds.
    let label_color = match bucket {
        StatusBucket::Active => COLOR_FRESH,
        StatusBucket::InFlight => COLOR_AGING,
        StatusBucket::Stale => COLOR_COLD,
        StatusBucket::Cold => COLOR_DIMMER,
    };
    let style = if is_selected {
        Style::default().fg(Color::Black).bg(COLOR_ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(label_color).add_modifier(Modifier::BOLD)
    };
    split_line(format!(" {label}"), format!("{count} "), width, style, style)
}

/// Truecolor gradient on silence age: fresh (<1m, still likely mid-turn) →
/// aging (<1h, worth a glance) → cold (≥1h, silent long enough to actually
/// worry about). SPEC.md §4 mandated the ANSI-16 palette "to inherit the
/// user's terminal theme" — a deliberate call made back when this doc was the
/// unstarted plan for a curses port; superseded by an explicit product
/// decision to go truecolor for a distinct, screenshot-worthy identity now
/// that ratatui is actually in use. The glyph *allowlist* (Unicode 1.1 only,
/// `petri/SPEC.md` §4) is a different, still-binding constraint — it exists
/// because of a real macOS `wcwidth` bug, not planning-doc caution.
fn silence_tier_color(secs: i64) -> Color {
    // Reuses the canonical Working/Recent/Idle thresholds
    // (`AGENT_WORKING_MAX_S` = 90s, `AGENT_RECENT_MAX_S` = 30m) rather than a
    // separate set of cutoffs invented for color alone — one silence
    // vocabulary for the whole app, not two that quietly disagree.
    match petridish_core::schema::agent_state_for_silence(secs) {
        AgentActivity::Working => COLOR_FRESH,
        AgentActivity::Recent => COLOR_AGING,
        AgentActivity::Idle => COLOR_COLD,
    }
}

/// Compact single-line row for a RUNNING project once the Dashboard has
/// dropped into the compact density tier (`COMPACT_TIER_MAX_CONTENT_ROWS`).
/// Same fields a roomy card carries, on one line: glyph, name, dirty marker,
/// branch, silence age — silence age still carries the gradient, since
/// "which run is stalling" is exactly what this row exists to answer at a
/// glance.
fn compact_running_row_line(radar: &Radar, proj_idx: usize, is_selected: bool) -> Line<'static> {
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

    let name_style = if is_selected {
        Style::default().fg(Color::Black).bg(COLOR_ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_FG).add_modifier(Modifier::BOLD)
    };
    let glyph_style = if is_selected { name_style } else { Style::default().fg(tier_color) };
    let dim = Style::default().fg(COLOR_DIM);
    let silence_style = if is_selected { name_style } else { Style::default().fg(tier_color) };

    Line::from(vec![
        Span::styled(format!(" {glyph} "), glyph_style),
        Span::styled(format!("{} ", fixed_width(&format!("{}{dirty_marker}", p.name), 26)), name_style),
        Span::styled(format!("{} ", fixed_width(branch, 22)), Style::default().fg(COLOR_BRANCH)),
        Span::styled(format!("{} ", fixed_width(&silence_str, 12)), silence_style),
        Span::styled(agent.to_string(), dim),
    ])
}

/// Roomy card for a project in the RUNNING section: three lines (each a
/// stable left value paired with the volatile right one) plus a blank
/// separator — petripy's actual roomy density (`format_card`), not a single
/// line crammed with every field.
fn roomy_card_lines(radar: &Radar, proj_idx: usize, is_selected: bool, width: usize) -> Vec<Line<'static>> {
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
    let right1 = if has_agent {
        match p.agent.active_agent.as_deref() {
            Some(agent) => format!("silent {} · {agent}", humanize_secs(silence_secs as u64)),
            None => format!("silent {}", humanize_secs(silence_secs as u64)),
        }
    } else {
        "no agent".to_string()
    };

    let branch = p.git.branch.as_deref().unwrap_or("-");
    let dirty_suffix = if dirty_marker.trim().is_empty() { String::new() } else { format!("  {}", dirty_marker.trim()) };
    let left2 = format!("     {branch}{dirty_suffix}");
    let right2 = p.agent.last_event.clone().unwrap_or_else(|| match p.git.last_commit_at {
        Some(dt) => format!("commit {}", commit_ago(dt)),
        None => "no commits".to_string(),
    });

    let display_path = abbreviate_home(&p.path);
    let left3 = format!("     {display_path}");
    let right3 = match p.agent.session_id.as_deref() {
        Some(session) => format!("sess {}", &session[..session.len().min(18)]),
        None => String::new(),
    };

    let tier_color = silence_tier_color(silence_secs);
    let name_style = if is_selected {
        Style::default().fg(Color::Black).bg(COLOR_ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_FG).add_modifier(Modifier::BOLD)
    };
    let silence_style = if is_selected { name_style } else { Style::default().fg(tier_color).add_modifier(Modifier::BOLD) };
    let dim = Style::default().fg(COLOR_DIM);

    let sparkline = sparkline_glyphs(&p.agent_activity, SPARKLINE_WIDTH);
    let left4 = Line::from(vec![
        Span::raw("     "),
        Span::styled(sparkline, Style::default().fg(tier_color)),
    ]);

    // Git's own activity timeline, deliberately separate from the agent sparkline above
    // (different cadence -- daily commits, not per-tick events -- and may split into its own
    // widget later per the redesign discussion). For now it just fills the otherwise-empty
    // middle of line 1, colored with the branch color rather than the silence gradient so the
    // two sparklines read as two different things at a glance, not two copies of one thing.
    let git_sparkline = sparkline_glyphs(&p.git.daily_commits, petridish_core::schema::GIT_ACTIVITY_WINDOW_DAYS);

    vec![
        split_line3(
            format!(" {glyph} {}{}{}", p.name, dirty_marker, uncommitted),
            git_sparkline,
            right1,
            width,
            name_style,
            Style::default().fg(COLOR_BRANCH),
            silence_style,
        ),
        split_line(left2, right2, width, dim, dim),
        split_line(left3, right3, width, dim, dim),
        left4,
        Line::from(""),
    ]
}

/// Unicode block elements U+2581-2588 ("Block Elements", standardized in Unicode 1.0/1.1) --
/// within petri's Unicode-1.1-only glyph rule (petri/SPEC.md §4, the real macOS `wcwidth`
/// gap that rule exists for), same rigor as every other glyph this module already renders.
const SPARKLINE_GLYPHS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// On-screen width (in samples) of the roomy-card agent sparkline -- deliberately narrower
/// than the full `AGENT_ACTIVITY_WINDOW` (60) ring; this is a glance-value shape indicator,
/// not a full-resolution chart.
const SPARKLINE_WIDTH: usize = 20;

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
fn compact_row_line(radar: &Radar, proj_idx: usize, is_selected: bool) -> Line<'static> {
    let p = &radar.projects[proj_idx];
    let branch = p.git.branch.as_deref().unwrap_or("(none)");
    let uncommitted = if p.git.uncommitted_files > 0 { format!("✎{}", p.git.uncommitted_files) } else { String::new() };
    let commit_age = match p.git.last_commit_at {
        Some(dt) => commit_ago(dt),
        None => "(none)".to_string(),
    };
    let gh = if p.git.github_url.is_some() { "[gh]" } else { "" };

    let style = if is_selected {
        Style::default().fg(Color::Black).bg(COLOR_ACCENT)
    } else {
        Style::default().fg(COLOR_FG)
    };

    Line::from(vec![
        Span::styled(format!(" {} ", fixed_width(&p.name, 26)), style),
        Span::styled(format!("{} ", fixed_width(branch, 22)), Style::default().fg(COLOR_BRANCH)),
        Span::styled(format!("{} ", fixed_width(&uncommitted, 4)), Style::default().fg(COLOR_DIM)),
        Span::styled(format!("{commit_age} "), Style::default().fg(COLOR_DIM)),
        Span::styled(gh, Style::default().fg(COLOR_ACCENT)),
    ])
}

/// Footer: the keymap advertising only keys actually bound (petri/SPEC.md §5).
fn footer_line() -> Line<'static> {
    Line::from(Span::styled(
        " j/k move  Space toggle  Enter open/browser  Tab Browser  q quit ",
        Style::default().fg(COLOR_DIM),
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
    fn split_line3_places_all_three_segments_in_order() {
        let line = split_line3(
            "L".to_string(),
            "M".to_string(),
            "R".to_string(),
            40,
            Style::default(),
            Style::default(),
            Style::default(),
        );
        let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let l_pos = rendered.find('L').unwrap();
        let m_pos = rendered.find('M').unwrap();
        let r_pos = rendered.find('R').unwrap();
        assert!(l_pos < m_pos, "left must precede middle: {rendered:?}");
        assert!(m_pos < r_pos, "middle must precede right: {rendered:?}");
    }

    #[test]
    fn split_line3_never_panics_when_content_overflows_width() {
        // left+middle+right longer than width -- must degrade to a wider-than-terminal
        // line (ratatui clips at render time), never panic on an underflowing subtraction.
        let line = split_line3(
            "x".repeat(30),
            "y".repeat(30),
            "z".repeat(30),
            10,
            Style::default(),
            Style::default(),
            Style::default(),
        );
        let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(rendered.contains('x') && rendered.contains('y') && rendered.contains('z'));
    }
}
