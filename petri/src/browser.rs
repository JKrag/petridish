//! The Browser screen (S5): grouped project list, selection, detail pane,
//! type-ahead filter. petri/SPEC.md §3.1 is the authoritative behavior
//! contract — read it in full before touching this file.

use crate::theme;
use petridish_core::present;
use petridish_core::schema::{AgentActivity, GitState, Project, Radar, StatusBucket};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};

use std::time::{SystemTime, UNIX_EPOCH};

/// Nerd Font glyphs for the git segment, drawn only when `nerd` is true.
///
/// **These are Private Use Area code points and will render as a tofu box for anyone
/// without a Nerd Font**, which is exactly why they sit behind `prefs::NerdFonts` and why
/// `glyph_portability.rs` carries them in a separate, opt-in allowlist rather than the main
/// one. Every glyph below is still held to that gate's width rule — one narrow cell — since
/// a two-cell glyph would corrupt the row's layout regardless of the font question.
///
/// Chosen for stability across Nerd Font versions: `U+E0A0` is the original Powerline
/// branch symbol and `U+F09B` is Font Awesome's github mark, both of which have survived
/// every renumbering the project has done (v3 moved large parts of the Octicons range,
/// which is why the more obvious `nf-oct-mark_github` is not used here).
const NERD_BRANCH: &str = "\u{e0a0}";
const NERD_GITHUB: &str = "\u{f09b}";
/// `nf-fa-folder_o` — a plain directory, for a project that is not a repository.
const NERD_NOT_A_REPO: &str = "\u{f114}";

/// Section order (petri/SPEC.md §3.1): "Grouped list, sections in the fixed
/// order active, in_flight, stale, cold". The Browser DOES render headers
/// with counts (§3.1) — they are just not selection stops here (selection
/// only ever lands on a project row; `BrowserState.visible` holds project
/// indices only, never a header). This is a Dashboard-only distinction (S6):
/// the Dashboard's headers ARE stops (§3.2), which needs its own cursor type
/// — do not reuse `BrowserState` for it. See browser::render for how a
/// header is drawn without being reachable by move_selection.
pub const SECTION_ORDER: [StatusBucket; 4] = [
    StatusBucket::Active,
    StatusBucket::InFlight,
    StatusBucket::Stale,
    StatusBucket::Cold,
];

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
    /// True while the `/` type-ahead input is open and taking keystrokes.
    ///
    /// This lives here, not in the event loop, because `render` needs it:
    /// the header chip has to distinguish "you are typing a query" (cursor,
    /// bright) from "a query is still applied" (no cursor, dim) — `ACT-10`.
    /// Keeping the loop's own copy of the flag alongside it would be two
    /// sources of truth for one mode, and the render would eventually
    /// disagree with the keymap.
    pub filter_input: bool,
    /// True while the `Space`-triggered detail overlay (issue #35) is open.
    /// Only consulted by `render` when the inline detail pane (beside or
    /// below the list, per `detail_placement`)
    /// isn't already showing — a terminal too narrow AND too short for
    /// either inline placement still needs some way to reach the
    /// detail-only fields (`github_url`, `session_id`, ...). Not modal:
    /// `j`/`k` etc. keep moving the selection while it's open, and the
    /// overlay's content just follows along, the same "live" feel the
    /// inline pane already has.
    pub detail_popup_open: bool,
}

impl BrowserState {
    /// Build the initial state from `radar`: exclude `is_foreign` projects,
    /// group by `SECTION_ORDER`, select the first visible row (or `None` if
    /// there are no visible projects at all). Unfiltered (`filter_query` is
    /// empty).
    pub fn new(radar: &Radar) -> Self {
        let visible = grouped_visible_indices(radar, "");
        let selected = if visible.is_empty() { None } else { Some(0) };
        Self {
            visible,
            selected,
            filter_query: String::new(),
            filter_input: false,
            detail_popup_open: false,
        }
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
        let previously_selected_project =
            self.selected.and_then(|pos| self.visible.get(pos).copied());

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
                    self.selected = if self.visible.is_empty() {
                        None
                    } else {
                        Some(0)
                    };
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
            if !query.is_empty() && !project.name.to_lowercase().contains(&query.to_lowercase()) {
                continue;
            }
            result.push(idx);
        }
    }
    result
}

/// Width threshold (inclusive): the detail pane is shown whenever the full
/// terminal width is at least this many columns. 65 leaves ~40 cols for the
/// detail pane after the list takes ~25, which is enough to read path, branch
/// and one field per row without squeezing — matching petri/SPEC.md §3.1's
/// "If the window is too narrow to give it a usable width, hide it entirely."
/// The acceptance test asserts this at 40 cols (must be absent), so any value
/// > 40 is acceptable; 65 gives a generous margin.
const DETAIL_PANE_THRESHOLD: u16 = 65;

/// Minimum usable content width for the detail pane (after subtracting the
/// scrollbar column). Below this we treat the detail pane as non-usable and
/// suppress it. 25 cols is plenty for path + one short field per row.
const DETAIL_PANE_DETAIL_MIN: u16 = 25;

/// Minimum OUTER height (borders included) for the detail pane once it is
/// stacked BELOW the list rather than beside it (issue #35: a terminal too
/// narrow for the side-by-side split, but tall enough to spend some of that
/// height on the pane instead of hiding it). 8 rows is 6 content rows (path,
/// branch, dirty, last commit, agent, last activity — the fields every
/// project has) plus top/bottom borders; the handful of optional fields
/// (`mine_last_commit_at`, `github_url`, `session_id`) may clip off the
/// bottom on a terminal that's only just tall enough to qualify for `Below`
/// at all — `below_detail_height` grows the pane past this floor whenever
/// there's more height to spend.
const DETAIL_PANE_BELOW_MIN_HEIGHT: u16 = 8;

/// OUTER height (borders included) that fits every possible detail field —
/// the two optional extras (`mine_last_commit_at`, `session_id`) plus the
/// always-present ones and `github_url`, 9 content rows, plus top/bottom
/// borders. `below_detail_height` never grows the pane past this: once every
/// field is visible there is nothing more for extra height to buy, and
/// growing further would just be giving the pane blank padding at the list's
/// expense.
const DETAIL_PANE_BELOW_MAX_HEIGHT: u16 = 11;

/// Minimum OUTER height the list must keep once the detail pane has taken
/// its slice off the bottom. Below this the list would show too few rows to
/// tell "scrolled" from "empty" apart, so stacking is abandoned entirely
/// (falls back to `Hidden`, reachable via the `Space` popup instead).
const LIST_MIN_HEIGHT_FOR_BELOW: u16 = 5;

/// Where the detail pane lands for a given main-area size — the three-way
/// choice `render` makes every frame. Mirrors petri/SPEC.md §3.1: beside the
/// list when there's room, below it when there's height AND enough width to
/// avoid squeezing it into a sliver (issue #35), and hidden — reachable only
/// via the `Space` popup overlay — when no placement is usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailPlacement {
    Side,
    Below,
    Hidden,
}

fn detail_placement(main: Rect) -> DetailPlacement {
    if main.width >= DETAIL_PANE_THRESHOLD {
        let list_width = (main.width * 2) / 3;
        let detail_and_scrollbar_w = main.width - list_width;
        let detail_width = detail_and_scrollbar_w.saturating_sub(1);
        if detail_width >= DETAIL_PANE_DETAIL_MIN {
            return DetailPlacement::Side;
        }
    }
    // `Below` still enforces `DETAIL_PANE_DETAIL_MIN` on the FULL width
    // (there's no separate list column to steal from, so the whole width is
    // available to the pane) — without this, a merely-tall terminal that's
    // also extremely narrow (e.g. 20 cols) would stack the pane into exactly
    // the unreadable sliver `DETAIL_PANE_DETAIL_MIN` exists to forbid.
    if main.width >= DETAIL_PANE_DETAIL_MIN
        && main.height >= LIST_MIN_HEIGHT_FOR_BELOW + DETAIL_PANE_BELOW_MIN_HEIGHT
    {
        return DetailPlacement::Below;
    }
    DetailPlacement::Hidden
}

/// The OUTER height actually given to a `Below`-placed detail pane: grows
/// past `DETAIL_PANE_BELOW_MIN_HEIGHT` to spend genuinely tall terminals'
/// extra height on more of the pane's fields (issue #35's "if the window is
/// tall enough, spend it" ask), capped at `DETAIL_PANE_BELOW_MAX_HEIGHT`
/// (every field already fits, so there's nothing further to buy) and at
/// leaving the list `LIST_MIN_HEIGHT_FOR_BELOW`. Pure geometry — no
/// project's actual field count is consulted — so `page_size` (`lib.rs`'s
/// `PageUp`/`PageDown`, which only has a terminal size to work with, not a
/// selected project) can mirror this exactly.
fn below_detail_height(main: Rect) -> u16 {
    main.height
        .saturating_sub(LIST_MIN_HEIGHT_FOR_BELOW)
        .clamp(DETAIL_PANE_BELOW_MIN_HEIGHT, DETAIL_PANE_BELOW_MAX_HEIGHT)
}

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
/// - At a narrow enough width AND short enough height, the detail pane must
///   be **absent entirely**, never squeezed into an unreadable sliver
///   (petri/SPEC.md §3.1 "If the window is too narrow to give it a usable
///   width, hide it entirely rather than squeezing") — reachable in that
///   case only via the `Space` popup overlay (issue #35). A terminal too
///   narrow for the side-by-side split but tall enough gets the pane
///   reflowed below the list instead of losing it outright.
/// - Must not panic on an empty `state.visible` (renders a "nothing
///   selected" state per petri/SPEC.md §3.1) or a degenerate 0×0/1×1 area.
pub fn render(frame: &mut Frame, radar: &Radar, state: &BrowserState, nerd: bool) {
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
    // treatment as the Dashboard's title, not just colored text — followed by
    // the filter chip when a filter is live (`ACT-10`).
    let mut header_spans = vec![Span::styled(
        " petri · browser ",
        Style::default()
            .fg(Color::Black)
            .bg(theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    )];
    // The badge is 17 columns; the chip gets what is left of the header row.
    header_spans.extend(filter_chip_spans(
        radar,
        state,
        area.width.saturating_sub(17),
    ));
    let header = Paragraph::new(Line::from(header_spans)).wrap(Wrap { trim: false });
    frame.render_widget(header, chunks[0]);

    let rule = Paragraph::new(Line::from(Span::styled(
        "═".repeat(area.width as usize),
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(rule, chunks[1]);

    let main = chunks[2];
    let placement = detail_placement(main);

    // Footer: the keymap (SPEC.md §5). The bar is "would a new user be misled?"
    // — don't advertise a key that does nothing — not a fixed list.
    let mut footer_text = String::from(" ");
    // `Space` (issue #35) only does anything observable when the inline
    // detail pane (beside or below) isn't already showing — advertising it
    // otherwise would be exactly the "would a new user be misled?" case the
    // rest of this footer avoids. Geometry-driven, like the detail pane's
    // own presence, not machine-driven, so it stays deterministic in tests.
    // Placed FIRST, not appended at the end: the footer row is
    // `Constraint::Length(1)` and never wraps, so anything past the right
    // edge is simply lost — and `Hidden` is, by construction, exactly the
    // narrow-or-short geometry where the tail is most likely to get clipped.
    // The one case `Space` is worth advertising at all is also the case a
    // trailing position couldn't be trusted to survive.
    if placement == DetailPlacement::Hidden {
        footer_text.push_str("Space detail  ");
    }
    footer_text.push_str(
        // The three action keys are advertised unconditionally, and deliberately
        // so — `g` always resolves thanks to ACT-3's git fallback, and `o`/`e`
        // always respond, with a notice when this machine or this project can't
        // satisfy them. Making the footer itself machine-dependent was
        // considered and rejected: it would make every snapshot test depend on
        // what happens to be installed on the machine running it.
        //
        // `o/O` is ACT-11's shifted re-pick variant, bound for every registry
        // action rather than just `g`: `O`'s list is thin today (one candidate,
        // `open`) but its `Other` row is exactly how a user pins a specific
        // browser, and a rule that holds for every key stays true as the
        // registry grows. PgUp/PgDn dropped from the advertisement to make
        // room — still bound, but the least discoverable-by-need of the set.
        "Tab Dashboard  j/k up-down  Shift+j/k ×10  / filter  o/O remote  g/G git log  e/E edit",
    );
    footer_text.push_str("  q quit ");
    let footer = Paragraph::new(Line::from(Span::styled(
        footer_text,
        Style::default().fg(theme::DIM),
    )))
    .wrap(Wrap { trim: false });
    frame.render_widget(footer, chunks[3]);

    // Main area: list plus detail pane, placed per `placement` — beside the
    // list when wide enough, below it when tall-but-narrow (issue #35), or
    // absent (reachable only via the `Space` popup) when neither fits.
    let (list_area, detail_inner): (Rect, Option<Rect>) = match placement {
        DetailPlacement::Side => {
            let list_width = (main.width * 2) / 3;
            let detail_and_scrollbar_w = main.width - list_width;
            let hsplit = Layout::horizontal([
                Constraint::Length(list_width),
                Constraint::Length(detail_and_scrollbar_w),
            ])
            .split(main);
            (hsplit[0], Some(hsplit[1]))
        }
        DetailPlacement::Below => {
            let detail_height = below_detail_height(main);
            let list_height = main.height - detail_height;
            let vsplit = Layout::vertical([
                Constraint::Length(list_height),
                Constraint::Length(detail_height),
            ])
            .split(main);
            (vsplit[0], Some(vsplit[1]))
        }
        DetailPlacement::Hidden => (main, None),
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
    // The INNER width: the bordered block spends one column on each side, and a row laid
    // out to the outer width would wrap.
    let inner_width = list_area.width.saturating_sub(2) as usize;
    let (list_lines, selected_line) = render_list_lines(radar, state, nerd, inner_width);
    let visible_rows = list_content_rows(list_area.height); // top + bottom border consumed
    let scroll_offset = compute_scroll_offset(selected_line, list_lines.len(), visible_rows);

    let list_para = Paragraph::new(list_lines)
        .block(Block::default().title(" Projects ").borders(Borders::ALL))
        .wrap(Wrap { trim: false })
        .scroll((scroll_offset as u16, 0));
    frame.render_widget(list_para, list_area);

    // Scrollbar: only for the side-by-side placement, where the horizontal
    // split leaves a spare column next to the list to put it in. When the
    // detail pane is stacked below instead (`Below`), there's no such spare
    // column without shrinking the list's own width by one everywhere else,
    // so the list falls back to scrolling without a visible thumb — the same
    // trade-off it already makes today whenever no detail pane renders at
    // all.
    if placement == DetailPlacement::Side
        && let Some(detail_area) = &detail_inner
    {
        let scrollbar_area = Rect {
            x: detail_area.x + detail_area.width - 1,
            y: detail_area.y,
            width: 1,
            height: detail_area.height,
        };
        let mut sb_state = ScrollbarState::new(state.visible.len()).position(scroll_offset);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut sb_state);
    }

    // Detail pane: beside or below the list, per `placement`.
    if let Some(detail_area) = detail_inner {
        render_detail_pane(frame, detail_area, radar, state);
    }

    // The `Space` popup (issue #35): the only way to reach the detail-only
    // fields when the window is too narrow AND too short for either inline
    // placement. Drawn last, over everything above, same `Clear`-then-draw
    // technique as `help::render` (MECH-1). Gated on `placement == Hidden` so
    // a stale `true` left over from a since-resized-wider terminal doesn't
    // draw a redundant second copy of the pane that's already inline.
    if placement == DetailPlacement::Hidden && state.detail_popup_open {
        render_detail_popup(frame, area, radar, state);
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
/// full terminal size (not the list block's own size — callers outside this
/// module, i.e. `lib.rs`'s key handler, only have `crossterm::terminal::size()`
/// to work with). Mirrors `render`'s own layout exactly: 3 rows of screen
/// chrome (header + heavy rule + footer, `render`'s `chunks`) surround the
/// main area, whose full height becomes the list block's OUTER height —
/// reduced further by `below_detail_height` when `detail_placement` says the
/// detail pane stacks below the list rather than beside it (issue #35: the
/// list/detail split isn't always horizontal-only any more, so `width` now
/// matters here too). `list_content_rows` then reduces that by the list
/// block's own border rows.
pub fn page_size(terminal_width: u16, terminal_height: u16) -> usize {
    let main_height = terminal_height.saturating_sub(3);
    let main = Rect {
        x: 0,
        y: 0,
        width: terminal_width,
        height: main_height,
    };
    let list_height = match detail_placement(main) {
        DetailPlacement::Below => main_height.saturating_sub(below_detail_height(main)),
        DetailPlacement::Side | DetailPlacement::Hidden => main_height,
    };
    list_content_rows(list_height)
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
    selected_line
        .saturating_sub(visible_rows - 1)
        .min(max_scroll)
}

/// The header's filter chip (`ACT-10`): the spans that follow the title badge
/// when a `/` filter is live, and nothing at all when it is not.
///
/// Two visually distinct states, because they mean different things:
///
/// - **Typing** (`filter_input`) — bright, with a block cursor after the
///   query. The mode is taking your keystrokes right now.
/// - **Applied but closed** (`Enter` pressed, query kept) — dim, no cursor.
///   This is the state `ACT-10` was actually about: a user who filters, looks
///   away, and looks back could not previously tell a filtered list from a
///   fleet that had gone quiet.
///
/// The match count is `matches of total`, where `total` is the *unfiltered*
/// visible row count — so `0 of 12` reads as "your query excluded everything",
/// which is the case that otherwise looks exactly like an empty radar.
///
/// This is deliberately in the header rather than replacing the footer keymap:
/// the footer stays useful *while* you filter, so swapping it for a filter
/// prompt would trade a permanently-useful surface for a transient one. The
/// header had the room.
fn filter_chip_spans(radar: &Radar, state: &BrowserState, avail: u16) -> Vec<Span<'static>> {
    if !state.filter_input && state.filter_query.is_empty() {
        return Vec::new();
    }

    let total = grouped_visible_indices(radar, "").len();
    let matched = state.visible.len();

    let (query_style, count_style) = if state.filter_input {
        (
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(theme::DIM),
        )
    } else {
        (
            Style::default().fg(theme::DIM),
            Style::default().fg(theme::DIMMER),
        )
    };

    let cursor = if state.filter_input { "\u{2588}" } else { "" };
    let count = format!("  {matched} of {total}");

    // The header row is `Constraint::Length(1)` — it cannot wrap, so anything
    // past the right edge is simply lost. The COUNT is the part that must
    // survive: `0 of 12` is the whole reason this chip exists (an empty
    // filtered list vs. an empty radar), and the query is the part the user
    // just typed and already knows. So the count gets its width first and the
    // query is truncated into whatever is left.
    let fixed = "  /".chars().count() + cursor.chars().count() + count.chars().count();
    let budget = (avail as usize).saturating_sub(fixed);
    let query = truncate_query(&state.filter_query, budget, state.filter_input);

    vec![
        Span::styled(format!("  /{query}{cursor}"), query_style),
        Span::styled(count, count_style),
    ]
}

/// Fit `query` into `budget` columns, marking the elision with `…`.
///
/// Which END is kept depends on the mode, and the difference is the point:
/// while the input is open the cursor sits after the last character typed, so
/// the TAIL is kept (`…rly-long-query`) and your keystrokes go on appearing.
/// Once the input is closed there is no cursor and nothing is arriving, so the
/// HEAD is kept (`a-fairly-lon…`), which is the half that identifies the query.
fn truncate_query(query: &str, budget: usize, keep_tail: bool) -> String {
    // Measured in COLUMNS, not characters. `budget` comes from the header's own width, and
    // a query with a wide character in it would otherwise overrun the chip and clip the
    // `<matched> of <total>` count — the one part of this chip the header is explicitly
    // laid out to protect.
    if crate::width::width(query) <= budget {
        return query.to_string();
    }
    if budget <= 1 {
        // No room for even one character plus the ellipsis. The count still
        // renders — losing the query entirely is the correct trade here.
        return "\u{2026}".chars().take(budget).collect();
    }
    if keep_tail {
        let tail = crate::width::take_width_end(query, budget - 1);
        format!("\u{2026}{tail}")
    } else {
        let head = crate::width::take_width(query, budget - 1);
        format!("{head}\u{2026}")
    }
}

/// Build the list's lines: section headers interleaved with project rows, in
/// SECTION_ORDER. Sections with 0 visible projects are skipped entirely.
/// Returns the lines plus the LINE index of the selected row (`None` if
/// nothing is selected) — the caller needs the real line position, not
/// `state.selected` (which only counts project rows, not the headers/blank
/// separators interleaved between them), to compute a correct scroll offset.
fn render_list_lines(
    radar: &Radar,
    state: &BrowserState,
    nerd: bool,
    avail: usize,
) -> (Vec<Line<'static>>, Option<usize>) {
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

    // Measured across every VISIBLE row, not the whole radar: a filtered list should lay
    // itself out for what it shows, and a project hidden by the filter must not reserve
    // width for a name nobody can see.
    let cols = {
        let mut widest_name = 0usize;
        let mut widest_git = 0usize;
        let mut widest_silence = 0usize;
        for &idx in &state.visible {
            let Some(p) = radar.projects.get(idx) else {
                continue;
            };
            let name = format!("{}{}", p.name, present::dirty_marker(&p.git).trim_end());
            widest_name = widest_name.max(crate::width::width(&name));
            widest_git = widest_git.max(crate::width::width(&git_segment(&p.git, nerd)));
            widest_silence =
                widest_silence.max(crate::width::width(&silence_display(p.last_activity_at)));
        }
        column_widths(widest_name, widest_git, widest_silence, avail)
    };

    let mut idx_in_visible = 0usize;

    // `is_last_populated_section` tracks whether any later section in the
    // iteration still has visible rows. We compute it by scanning ahead.
    let section_is_last = |target: &StatusBucket| -> bool {
        SECTION_ORDER
            .iter()
            .skip_while(|&s| *s != *target)
            .any(|&s| {
                radar
                    .projects
                    .iter()
                    .any(|p| p.status_bucket == s && !p.is_foreign)
                    && state.visible.iter().any(|&idx| {
                        idx < radar.projects.len() && radar.projects[idx].status_bucket == s
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
            lines.push(render_project_row(radar, proj_idx, is_selected, nerd, cols));
            idx_in_visible += 1;
        }

        // Blank separator between non-last populated sections.
        if !section_is_last(section) {
            lines.push(Line::from(""));
        }
    }

    (lines, selected_line)
}

/// The git segment for one row (issue #38): is this a repository at all, and if so what is
/// uncommitted in it.
///
/// **Deliberately carries no branch name.** A branch name is unbounded — `feature/JIRA-1234-
/// rework-the-thing` is ordinary — and this list is often only ~25 columns wide with the
/// detail pane beside it, so a branch would either dominate the row or need its own
/// truncation ladder. The branch is one keypress away in the detail pane and the focus
/// panel, both of which have the room for it.
///
/// `!N` modified, `?N` untracked — the vocabulary of `git status --short`, starship and
/// lazygit, so it needs no legend for anyone who uses git. Zero counts are omitted rather
/// than shown as `!0`: a clean repo has nothing to say.
///
/// `nerd` swaps the leading marker for a Nerd Font glyph. The counts are identical in both
/// modes, so the information never depends on the font — only the decoration does.
pub(crate) fn git_segment(git: &GitState, nerd: bool) -> String {
    if !git.is_repo {
        // The non-repo marker is the whole of #38's first ask, so it is a *positive* mark
        // rather than an empty cell: "not a repo" and "clean repo" must not look alike.
        return if nerd {
            NERD_NOT_A_REPO.to_string()
        } else {
            "-".to_string()
        };
    }

    let modified = git.uncommitted_files.saturating_sub(git.untracked_files);
    let mut out = String::new();

    if nerd {
        // The host icon when we know the host, the generic branch glyph otherwise. Same
        // shape as Kasper's screenshot on #38, which leads with the GitHub mark.
        out.push_str(if is_github(git) {
            NERD_GITHUB
        } else {
            NERD_BRANCH
        });
    }

    if modified > 0 {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!("!{modified}"));
    }
    if git.untracked_files > 0 {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!("?{}", git.untracked_files));
    }
    out
}

/// Is the remote GitHub?
///
/// `swab::git::github_url` already normalises every remote it accepts to
/// `https://github.com/OWNER/REPO` and returns `None` for anything else, so in practice
/// `github_url.is_some()` would answer this. The host is checked anyway, and matched on the
/// *host portion* rather than by searching the string: `petri` reads a state file it did
/// not write, and this keeps a mirror at `git.example.com/github-backups/x` from borrowing
/// the mark if that normalisation is ever relaxed. The SSH form is deliberately not handled
/// — the writer rewrites `git@github.com:` to the https form before it is ever stored.
fn is_github(git: &GitState) -> bool {
    git.github_url.as_deref().is_some_and(|url| {
        url.split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(url)
            .split('/')
            .next()
            == Some("github.com")
    })
}

/// Columns spent before the name: the leading glyph and the spaces around it.
const ROW_LEADER_COLS: usize = 3;
/// Blank columns between two adjacent cells.
const COLUMN_GAP: usize = 2;
/// The narrowest the name column may be squeezed to before the columns to its right start
/// giving up width instead. Below this a name is mostly ellipsis and identifies nothing.
const NAME_COLUMN_MIN: usize = 6;

/// The widest the name column may grow, however long the longest name is.
///
/// Without a cap, one outlier name pads *every* row out to its length — and it does not
/// even have to be on screen, since the column is measured across the whole visible list
/// while only a screenful is drawn. Observed on real data: a list of `alpha-NN` rows sat
/// with eleven dead columns before the git cell because of a longer name in a section
/// further down. 28 columns clears every project name in the author's own fleet
/// (`devops-academy-handins` is 22), so the cap costs nothing in the common case and
/// bounds the damage in the uncommon one; anything longer is ellipsised by `fit_exact` and
/// still readable in the detail pane.
const NAME_COLUMN_MAX: usize = 28;

/// The three data columns' widths for one render pass, in display columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RowColumns {
    pub(crate) name: usize,
    pub(crate) git: usize,
    pub(crate) silence: usize,
}

/// Fit the three columns into the pane, shrinking under pressure.
///
/// **Every row is laid out to exactly the same total width**, which is what makes the git
/// and age cells line up down the list instead of floating after names of differing length.
/// That total must never exceed the pane: the list is drawn by a `Paragraph` with
/// `Wrap { trim: false }`, so a row one column too wide does not clip — it *wraps*, and a
/// wrapped row pushes every row below it out of step with the scroll maths, which counts
/// lines. Hence a shrink ladder rather than a plain max.
///
/// The order is deliberate. The name gives way first, because it is the only cell that
/// degrades gracefully — `petridish-cl…` still identifies a project — down to
/// `NAME_COLUMN_MIN`. The git segment goes next. The age gives way last: it is the shortest
/// cell and the one truncation destroys rather than degrades, since the unit is the final
/// character and `20d ago` cut to `20d` is a different claim.
pub(crate) fn column_widths(
    widest_name: usize,
    widest_git: usize,
    widest_silence: usize,
    avail: usize,
) -> RowColumns {
    let fixed = ROW_LEADER_COLS + COLUMN_GAP * 2;
    let mut cols = RowColumns {
        name: widest_name.min(NAME_COLUMN_MAX),
        git: widest_git,
        silence: widest_silence,
    };
    let over = |c: &RowColumns| (fixed + c.name + c.git + c.silence).saturating_sub(avail);

    let excess = over(&cols);
    if excess > 0 {
        cols.name = cols.name.saturating_sub(excess).max(NAME_COLUMN_MIN);
    }
    let excess = over(&cols);
    if excess > 0 {
        cols.git = cols.git.saturating_sub(excess);
    }
    let excess = over(&cols);
    if excess > 0 {
        cols.silence = cols.silence.saturating_sub(excess);
    }
    // A pane too narrow for even the floor: the name gives up the remainder. Nothing may
    // return a total wider than `avail`, or the row wraps.
    let excess = over(&cols);
    if excess > 0 {
        cols.name = cols.name.saturating_sub(excess);
    }
    cols
}

/// Right-aligned cell, for the age column: the unit is the last character, and a ragged
/// right edge is what makes a column of ages hard to compare at a glance.
fn cell_right(text: &str, w: usize) -> String {
    let text = crate::width::take_width(text, w);
    let pad = w.saturating_sub(crate::width::width(&text));
    format!("{}{}", " ".repeat(pad), text)
}

/// Render one project row: glyph (● working / ○ otherwise), name with its dirty marker,
/// the git segment, and the silence age — each in the column width `cols` allots it, so the
/// cells line up down the list. Selection highlight applied via `is_selected`.
fn render_project_row(
    radar: &Radar,
    proj_idx: usize,
    is_selected: bool,
    nerd: bool,
    cols: RowColumns,
) -> Line<'static> {
    let project = &radar.projects[proj_idx];

    let glyph = match project.agent.state {
        AgentActivity::Working => "●",
        _ => "○",
    };

    // The dirty marker is a suffix on the name, not a column of its own — it is one
    // character and belongs against the thing it qualifies. `dirty_marker` pads to a space
    // for a clean repo, which would otherwise widen every name cell by one.
    let name = format!(
        "{}{}",
        project.name,
        present::dirty_marker(&project.git).trim_end()
    );

    let git = git_segment(&project.git, nerd);

    let silence = silence_display(project.last_activity_at);

    // Selection = reverse video (black on accent), bold — the same
    // convention `dashboard.rs`'s `solid_selected_line` uses for its compact
    // rows: "reverse video is the canonical current-selection signal"
    // (`references/visual-patterns.md`), applied as a background fill rather
    // than a text-color shift so it reads as a bar, not just a tint — the
    // same reason `not-selected` uses `FG` (near-white) rather than a color
    // close enough to `ACCENT` to blur the two states together.
    let style = if is_selected {
        Style::default()
            .fg(Color::Black)
            .bg(theme::ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::FG)
    };
    let meta_style = if is_selected {
        style
    } else {
        Style::default().fg(theme::DIM)
    };

    let gap = " ".repeat(COLUMN_GAP);
    Line::from(vec![
        Span::styled(format!(" {} ", glyph), style),
        Span::styled(crate::width::fit_exact(&name, cols.name), style),
        Span::styled(gap.clone(), meta_style),
        Span::styled(crate::width::fit_exact(&git, cols.git), meta_style),
        Span::styled(gap, meta_style),
        Span::styled(cell_right(&silence, cols.silence), meta_style),
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
fn render_detail_pane(frame: &mut Frame, area: Rect, radar: &Radar, state: &BrowserState) {
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
    let para = Paragraph::new(body_lines)
        .block(detail_block)
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

/// The `Space` detail popup (issue #35): a centred overlay carrying the same
/// content as `render_detail_pane`, for terminals too narrow AND too short
/// for either inline placement. Same `Clear`-then-draw technique as
/// `help::render` (MECH-1) — must be called last in the frame. Not modal:
/// unlike the help popup, `render` calling this is the only wiring here —
/// dismissal and content updates while it's open are the event loop's job
/// (`lib.rs`), so navigation keeps working underneath it, live, the same way
/// the inline pane already tracks `state.selected` as it changes.
fn render_detail_popup(frame: &mut Frame, area: Rect, radar: &Radar, state: &BrowserState) {
    use ratatui::layout::Flex;
    use ratatui::widgets::Clear;

    let (title, body_lines): (&str, Vec<Line<'static>>) = match state.selected_project(radar) {
        Some(project) => (" Detail ", render_detail_lines(project)),
        None => (
            " Detail ",
            vec![Line::from(Span::styled(
                "  No project selected",
                Style::default().fg(theme::DIM),
            ))],
        ),
    };

    // Unlike the inline pane (competing with the list for width), the popup
    // is the only thing on screen — so it takes as much of the frame as it
    // reasonably can, capped at 70 (a github URL fits comfortably inside
    // that) rather than the inline pane's much tighter `DETAIL_PANE_DETAIL_MIN`.
    let width = 70.min(area.width.saturating_sub(4)).max(20);
    let height = (body_lines.len() as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(3);
    let [popup] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(popup);

    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(body_lines).wrap(Wrap { trim: false }), inner);
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
            format!("  Dirty: {} uncommitted", project.git.uncommitted_files),
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
    match (
        &project.git.last_commit_at,
        &project.git.mine_last_commit_at,
    ) {
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
    if let Ok(home) = std::env::var("HOME")
        && let Ok(p) = std::path::Path::new(path).strip_prefix(&home)
    {
        let remainder = p.to_string_lossy().to_string();
        if remainder.is_empty() {
            return home;
        }
        let start = if remainder.starts_with('/') { "" } else { "/" };
        return format!("~{}{}", start, remainder);
    }
    path.to_string()
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

#[cfg(test)]
mod tests {

    use super::{git_segment, is_github};

    fn repo(uncommitted: u32, untracked: u32, url: Option<&str>) -> GitState {
        GitState {
            is_repo: true,
            branch: Some("main".to_string()),
            is_dirty: uncommitted > 0,
            uncommitted_files: uncommitted,
            untracked_files: untracked,
            last_commit_at: None,
            mine_last_commit_at: None,
            github_url: url.map(str::to_string),
            daily_commits: Vec::new(),
        }
    }

    #[test]
    fn every_row_is_laid_out_to_the_same_width() {
        // The alignment property itself, and the safety property behind it: the list is
        // drawn with `Wrap { trim: false }`, so a row wider than the pane wraps rather than
        // clips and takes the scroll maths with it.
        for avail in 8..80 {
            let cols = super::column_widths(30, 6, 7, avail);
            let total = super::ROW_LEADER_COLS
                + cols.name
                + super::COLUMN_GAP * 2
                + cols.git
                + cols.silence;
            assert!(
                total <= avail,
                "columns {cols:?} total {total} exceed the pane's {avail}"
            );
        }
    }

    #[test]
    fn one_outlier_name_does_not_pad_every_row() {
        // The cap exists because the column is measured across the whole visible list while
        // only a screenful is drawn, so an off-screen name could widen every row on screen.
        let cols = super::column_widths(120, 6, 7, 200);
        assert_eq!(cols.name, super::NAME_COLUMN_MAX);
    }

    #[test]
    fn a_roomy_pane_gives_every_column_what_it_asked_for() {
        let cols = super::column_widths(20, 6, 7, 100);
        assert_eq!(
            cols,
            super::RowColumns {
                name: 20,
                git: 6,
                silence: 7
            }
        );
    }

    #[test]
    fn the_name_gives_up_width_before_the_other_columns_do() {
        // Order matters: the name is the only cell that degrades gracefully, and the age is
        // the one truncation destroys outright ("20d ago" -> "20d" is a different claim).
        let cols = super::column_widths(28, 6, 7, 40);
        assert!(cols.name < 28, "the name must have been squeezed");
        assert_eq!(
            cols.git, 6,
            "git keeps its width while the name can still give"
        );
        assert_eq!(cols.silence, 7, "and the age is untouched");
    }

    #[test]
    fn a_pane_too_narrow_for_the_floor_still_fits() {
        // Below `NAME_COLUMN_MIN` everything gives way in turn rather than overflowing.
        let cols = super::column_widths(30, 6, 7, 12);
        let total =
            super::ROW_LEADER_COLS + cols.name + super::COLUMN_GAP * 2 + cols.git + cols.silence;
        assert!(total <= 12, "got {cols:?} totalling {total}");
    }

    #[test]
    fn the_git_and_age_cells_start_at_the_same_column_on_every_row() {
        // The end-to-end version of the property: render real rows whose names differ in
        // length and assert the columns line up in the buffer.
        let mut short = project("s", "short", StatusBucket::Active);
        short.git = repo(1, 0, None);
        let mut long = project("l", "a-much-longer-project-name", StatusBucket::Active);
        long.git = repo(2, 0, None);
        let radar = radar_of(vec![short, long]);
        let state = BrowserState::new(&radar);
        let (lines, _) = super::render_list_lines(&radar, &state, false, 60);

        let rows: Vec<String> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .filter(|t| t.contains("short") || t.contains("a-much-longer-project-name"))
            .collect();
        assert_eq!(rows.len(), 2, "expected both project rows, got {rows:?}");

        let marker_col = |row: &str, needle: &str| -> usize {
            crate::width::width(&row[..row.find(needle).expect("cell must be present")])
        };
        assert_eq!(
            marker_col(&rows[0], "!1"),
            marker_col(&rows[1], "!2"),
            "git cells must start at the same column:\n{}\n{}",
            rows[0],
            rows[1]
        );
        assert_eq!(
            crate::width::width(&rows[0]),
            crate::width::width(&rows[1]),
            "rows must be equally wide"
        );
    }

    #[test]
    fn a_non_repo_is_marked_positively_not_by_an_empty_cell() {
        // Issue #38's first ask. "Not a repo" and "clean repo" must not look alike, which
        // rules out simply leaving the cell blank for one of them.
        let plain = git_segment(&GitState::not_a_repo(), false);
        let clean = git_segment(&repo(0, 0, None), false);
        assert_eq!(plain, "-");
        assert_ne!(plain, clean, "the two states must be distinguishable");
    }

    #[test]
    fn a_clean_repo_says_nothing() {
        assert_eq!(git_segment(&repo(0, 0, None), false), "");
    }

    #[test]
    fn modified_and_untracked_are_counted_apart() {
        // 3 total of which 2 untracked -> 1 modified. The subtraction is the whole reason
        // `untracked_files` is stored as a subset rather than a second total.
        assert_eq!(git_segment(&repo(3, 2, None), false), "!1 ?2");
    }

    #[test]
    fn a_zero_count_is_omitted_rather_than_shown_as_zero() {
        assert_eq!(git_segment(&repo(2, 0, None), false), "!2");
        assert_eq!(git_segment(&repo(2, 2, None), false), "?2");
    }

    #[test]
    fn the_counts_do_not_depend_on_the_font() {
        // The decoration changes with `nerd`; the information must not.
        let git = repo(3, 2, None);
        let ascii = git_segment(&git, false);
        let nerd = git_segment(&git, true);
        assert!(nerd.contains("!1") && nerd.contains("?2"), "got {nerd:?}");
        assert!(
            ascii.contains("!1") && ascii.contains("?2"),
            "got {ascii:?}"
        );
        assert!(!nerd.is_ascii(), "nerd mode should add a glyph: {nerd:?}");
        assert!(ascii.is_ascii(), "ascii mode must stay ascii: {ascii:?}");
    }

    #[test]
    fn the_host_mark_is_used_only_for_a_real_github_remote() {
        let gh = git_segment(
            &repo(1, 0, Some("https://github.com/JKrag/petridish")),
            true,
        );
        let other = git_segment(&repo(1, 0, Some("https://gitlab.com/x/y")), true);
        assert_ne!(gh, other, "a non-github remote must not borrow the mark");

        assert!(is_github(&repo(0, 0, Some("https://github.com/a/b"))));
        assert!(!is_github(&repo(0, 0, None)));
        // The SSH form is not asserted as supported on purpose: `swab::git::github_url`
        // rewrites `git@github.com:a/b.git` to the https form before storing it, so that
        // shape never reaches `petri` and pretending to handle it would be untested code
        // pinned by an untrue test.
        assert!(
            !is_github(&repo(
                0,
                0,
                Some("https://git.example.com/github-backups/x")
            )),
            "matched on the host, not on the string containing `github` anywhere"
        );
        assert!(
            !is_github(&repo(0, 0, Some("https://notgithub.com/a/b"))),
            "a host that merely ends in the same letters is a different host"
        );
    }
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
        let (_, selected_line) = render_list_lines(&radar, &state, false, 60);
        assert_eq!(
            selected_line,
            Some(1),
            "the header line must be accounted for"
        );

        state.move_selection(1);
        let (_, selected_line) = render_list_lines(&radar, &state, false, 60);
        assert_eq!(
            selected_line,
            Some(2),
            "moving selection by one project row must move the line index by one, not reset relative to the header"
        );

        state.move_selection(1);
        let (_, selected_line) = render_list_lines(&radar, &state, false, 60);
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
            projects.push(project(
                &format!("r{i}"),
                &format!("running-{i}"),
                StatusBucket::Active,
            ));
        }
        for i in 0..10 {
            projects.push(project(
                &format!("f{i}"),
                &format!("flight-{i}"),
                StatusBucket::InFlight,
            ));
        }
        let radar = radar_of(projects);
        let mut state = BrowserState::new(&radar);

        // Move to the very last project row.
        for _ in 0..20 {
            state.move_selection(1);
        }

        let (list_lines, selected_line) = render_list_lines(&radar, &state, false, 60);
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
        use ratatui::{Terminal, backend::TestBackend};

        // Enough project rows that a small terminal can't show them all at
        // once, so a scroll-lag bug has room to manifest.
        let projects: Vec<Project> = (0..20)
            .map(|i| {
                project(
                    &format!("p{i}"),
                    &format!("project-{i}"),
                    StatusBucket::Active,
                )
            })
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
        terminal
            .draw(|frame| render(frame, &radar, &state, false))
            .expect("draw must not error");
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
    /// rule, footer) plus the list block's own 2 border rows, 5 total. Width
    /// 80 keeps the detail pane (if any) beside the list, so it never eats
    /// into the list's height here.
    #[test]
    fn page_size_accounts_for_screen_chrome_and_list_borders() {
        // 24-row terminal: 24 - 3 (header/rule/footer) - 2 (list borders) = 19.
        assert_eq!(page_size(80, 24), 19);
        // 80x50: 50 - 5 = 45.
        assert_eq!(page_size(80, 50), 45);
        // Degenerate terminals must floor at 1, never 0 (a 0-row page jump
        // would be a silent no-op) and must never underflow/panic.
        assert_eq!(page_size(80, 3), 1);
        assert_eq!(page_size(80, 0), 1);
    }

    /// A narrow-but-tall terminal (issue #35) stacks the detail pane below
    /// the list instead of hiding it, which eats `below_detail_height` rows
    /// out of the list's own height — `page_size` must reflect that, or
    /// `PageDown` would jump the selection past what's actually visible.
    /// Covers both ends of `below_detail_height`'s range: pinned at its
    /// floor on a terminal that only just qualifies for `Below`, grown to
    /// its cap on a genuinely tall one.
    #[test]
    fn page_size_accounts_for_a_stacked_detail_pane() {
        // 40 wide (below DETAIL_PANE_THRESHOLD) x 16 tall: main height =
        // 16 - 3 = 13, exactly `LIST_MIN_HEIGHT_FOR_BELOW` (5) +
        // `DETAIL_PANE_BELOW_MIN_HEIGHT` (8) — the smallest terminal that
        // still qualifies for `Below` at all, so the detail pane sits at
        // its floor (8): list height = 13 - 8 = 5, minus 2 list borders = 3.
        assert_eq!(page_size(40, 16), 3);
        // 40 wide x 30 tall: main height = 27, well past the floor, so the
        // detail pane grows to its cap (`DETAIL_PANE_BELOW_MAX_HEIGHT`, 11):
        // list height = 27 - 11 = 16, minus 2 list borders = 14.
        assert_eq!(page_size(40, 30), 14);
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
            projects.push(project(
                &format!("r{i}"),
                &format!("running-{i}"),
                StatusBucket::Active,
            ));
        }
        for i in 0..10 {
            projects.push(project(
                &format!("f{i}"),
                &format!("flight-{i}"),
                StatusBucket::InFlight,
            ));
        }
        let radar = radar_of(projects);
        let mut state = BrowserState::new(&radar);
        let visible_rows = 14usize; // comfortably fits the header + first few rows

        // Move down twice — well within the visible window — and confirm no
        // scrolling happened at all.
        for _ in 0..2 {
            state.move_selection(1);
            let (list_lines, selected_line) = render_list_lines(&radar, &state, false, 60);
            let scroll_offset =
                compute_scroll_offset(selected_line, list_lines.len(), visible_rows);
            assert_eq!(
                scroll_offset, 0,
                "selection at line {selected_line:?} still fits within {visible_rows} visible rows — must not scroll yet"
            );
        }
    }
}
