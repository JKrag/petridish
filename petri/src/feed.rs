//! SPACE-1 — the fleet activity feed.
//!
//! The Dashboard's leftover vertical space (see `petri/IDEAS.md` §3's diagnosis: nothing in
//! the layout ever *grows* to consume surplus height) is filled with a scrolling record of
//! what the fleet has been doing: `14:22  project-radar · claude-code stop · 3 files`.
//!
//! **Where the events come from, and why not `events.ndjson`.** `IDEAS.md`'s SPACE-1 entry
//! proposes reading `~/.petridish/events.ndjson` directly, since `swab-hook` already writes
//! it. That file cannot serve this: `swab::events::read_and_compact` **truncates it on every
//! scan tick** (events are consumed exactly once, by design — it is a hand-off buffer to the
//! scanner, not a log), so at any moment it holds at most one tick's worth of events and a
//! reader would also be racing the scanner's truncation. The durable record of the same
//! information is `projects.json` itself: `Project::agent.last_event`/`last_event_at`,
//! `git.last_commit_at` and `status_bucket` all carry forward tick to tick. So the feed is
//! derived by **diffing successive `Radar` snapshots** — `ingest(prev, next)` — which is
//! race-free, needs no second file, and keeps `petri` a pure reader of the state file.
//!
//! Two honest consequences of that choice:
//!
//! 1. The feed advances at **scan cadence**, not in real time — nothing here should be
//!    labelled "live".
//! 2. Between two ticks a project's agent may have fired many events; the snapshot only
//!    carries the newest. One row per project per tick is the resolution available.
//!
//! Agent-agnostic by construction (`IDEAS.md` §5): rows name whatever `active_agent` says,
//! never "Claude".

use crate::theme;
use chrono::{DateTime, Utc};
use petridish_core::present::status_bucket_str;
use petridish_core::schema::{Project, Radar};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::{HashMap, VecDeque};

/// How many rows the feed remembers. Older rows are dropped from the back as new ones
/// arrive at the front. Sized well past any plausible on-screen row count so that
/// scrollback exists for a later `SURF-1` timeline screen to draw on, while still bounding
/// a long-running `petri`'s memory.
pub const FEED_CAPACITY: usize = 200;

/// What kind of change produced a feed row. Carried so a renderer can colour rows by kind
/// without re-parsing `detail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FeedKind {
    /// `agent.last_event_at` advanced — the agent did something.
    Agent,
    /// `git.last_commit_at` advanced — a commit landed.
    Commit,
    /// `status_bucket` changed — the project moved between sections.
    Bucket,
    /// The project was not in the previous snapshot at all.
    Appeared,
    /// `agent.waiting_since` started a new latch — the agent is blocked on a human
    /// (`MECH-5`). Appended last so the existing `Ord`-derived tie-break between kinds keeps
    /// the order it had.
    Waiting,
}

/// One row of the feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedEvent {
    /// When the underlying thing happened. For `Agent`/`Commit` this is the schema's own
    /// timestamp for that event; for `Bucket`/`Appeared` (which have no timestamp of their
    /// own) it is the `Radar::updated_at` of the snapshot that revealed the change.
    pub at: DateTime<Utc>,
    /// `Project::name`.
    pub project: String,
    pub kind: FeedKind,
    /// The already-humanised tail of the row, e.g. `"claude-code stop · 3 files"`.
    pub detail: String,
}

impl FeedEvent {
    /// The row as one line of text: `"HH:MM  {project} · {detail}"`, with the clock in UTC
    /// at minute precision.
    ///
    /// UTC rather than local time is deliberate and matches the Dashboard header, which
    /// already formats `chrono::Utc::now()` as `%H:%M` (`dashboard::header_lines`) — a feed
    /// on a different clock from the header directly above it would be worse than either
    /// choice on its own.
    ///
    /// **An event from an earlier day shows `MM-DD` instead of a clock**, in the same
    /// five-column field so the rows stay aligned. Found by running the feed against a real
    /// `projects.json`: a fleet that has been quiet overnight renders `23:14` *below*
    /// `04:58`, which reads as a sorting bug when the order is in fact correct. A bare clock
    /// simply cannot express "yesterday", and the feed's whole purpose is chronology.
    pub fn row_text(&self, now: DateTime<Utc>) -> String {
        format!("{}  {}", self.stamp(now), self.body_text())
    }

    /// True when this event happened on `now`'s UTC date — i.e. its stamp is a clock
    /// rather than a date.
    pub fn is_today(&self, now: DateTime<Utc>) -> bool {
        self.at.date_naive() == now.date_naive()
    }

    /// The five-column time field: `HH:MM` for today, `MM-DD` for any earlier day.
    pub fn stamp(&self, now: DateTime<Utc>) -> String {
        if self.is_today(now) {
            self.at.format("%H:%M").to_string()
        } else {
            self.at.format("%m-%d").to_string()
        }
    }

    /// Everything after the stamp: `"{project} · {detail}"`.
    pub fn body_text(&self) -> String {
        format!("{} · {}", self.project, self.detail)
    }
}

/// Turn a raw hook event name into something readable: `"PreToolUse"` -> `"pre tool use"`,
/// `"Stop"` -> `"stop"`, `"user_prompt_submit"` -> `"user prompt submit"`.
///
/// Splits on `_` and on lowercase->uppercase boundaries, lowercases every part, and joins
/// with single spaces. Runs of consecutive capitals stay together (`"HTTPStart"` ->
/// `"http start"`). An input that yields nothing at all (empty, or only separators) falls
/// back to `"activity"` so a row is never left with a dangling `·`.
pub fn humanize_event(raw: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    for segment in raw.split('_') {
        if segment.is_empty() {
            continue;
        }
        let b = segment.as_bytes();
        let n = b.len();
        let mut start = 0usize;
        for i in 1..n {
            let cur = b[i];
            if cur.is_ascii_uppercase() {
                // A word boundary sits before an uppercase letter when it follows a
                // lowercase one (camelCase) or ends a run of capitals ("HTTPStart").
                let prev_lower = b[i - 1].is_ascii_lowercase();
                let next_lower = i + 1 < n && b[i + 1].is_ascii_lowercase();
                if prev_lower || next_lower {
                    words.push(segment[start..i].to_ascii_lowercase());
                    start = i;
                }
            }
        }
        words.push(segment[start..].to_ascii_lowercase());
    }
    let joined = words.join(" ");
    if joined.is_empty() {
        "activity".to_string()
    } else {
        joined
    }
}

/// The `detail` string for an agent event on `p`: `"{agent} {event}"`, plus a
/// `" · {n} file(s)"` suffix when the project is a dirty repo.
///
/// `active_agent` missing falls back to the literal `"agent"` (agent-agnostic — never a
/// specific vendor), `last_event` missing falls back to `humanize_event`'s `"activity"`.
/// The file count is singular at 1 (`"1 file"`), plural otherwise, and is omitted entirely
/// when the project is not a repo or has no uncommitted files.
pub fn agent_detail(p: &Project) -> String {
    let agent = p.agent.active_agent.clone().unwrap_or("agent".to_string());
    let event = humanize_event(p.agent.last_event.as_deref().unwrap_or(""));
    let mut detail = format!("{agent} {event}");
    // A dirty repo appends how many uncommitted files sit on it. Keyed on the COUNT, not
    // on `is_dirty`: the scanner derives the two independently, and the suffix reports the
    // number — so a flag/count disagreement must drop the suffix rather than render
    // "· 0 files".
    let count = p.git.uncommitted_files;
    if p.git.is_repo && count > 0 {
        let unit = if count == 1 { "file" } else { "files" };
        detail.push_str(&format!(" · {count} {unit}"));
    }
    detail
}

/// The rolling activity feed. Newest event at the front.
#[derive(Debug, Clone, Default)]
pub struct FeedState {
    events: VecDeque<FeedEvent>,
}

impl FeedState {
    /// Build the initial feed from a single snapshot, so a freshly-started `petri` shows
    /// something rather than an empty box until the next scan tick.
    ///
    /// One `FeedKind::Agent` row per project that has an `agent.last_event_at`, at that
    /// timestamp, with `agent_detail`'s text; projects with no agent timestamp contribute
    /// nothing. Ordered newest-first, ties broken by project name ascending so the result
    /// is deterministic. Truncated to `FEED_CAPACITY`.
    ///
    /// These rows are a snapshot fanned out, not recovered history — each says "this is the
    /// last thing this project did", which is exactly what it claims on screen.
    pub fn seeded(radar: &Radar) -> Self {
        // One Agent row per project with an agent timestamp, then newest-first.
        let mut events: Vec<FeedEvent> = Vec::new();
        for p in &radar.projects {
            if let Some(at) = p.agent.last_event_at {
                events.push(FeedEvent {
                    at,
                    project: p.name.clone(),
                    kind: FeedKind::Agent,
                    detail: agent_detail(p),
                });
            }
        }
        events.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.project.cmp(&b.project)));
        let mut state = FeedState::default();
        for e in events {
            state.events.push_back(e);
        }
        while state.events.len() > FEED_CAPACITY {
            state.events.pop_back();
        }
        state
    }

    /// Fold the difference between two consecutive snapshots into new rows at the front.
    ///
    /// Projects are matched by `Project::id`. For each project in `next`:
    ///
    /// - **not present in `prev`** -> one `Appeared` row at `next.updated_at`, detail
    ///   `"discovered"`.
    /// - otherwise, in this order, and a single project may produce more than one row:
    ///   - `agent.last_event_at` strictly greater than before -> `Agent` row at the new
    ///     timestamp, detail `agent_detail(p)`. (`None` counts as older than any `Some`.)
    ///   - `git.last_commit_at` strictly greater than before -> `Commit` row at the new
    ///     timestamp, detail `"commit on {branch}"`, branch falling back to `"detached"`.
    ///   - `status_bucket` different from before -> `Bucket` row at `next.updated_at`,
    ///     detail `"{old} → {new}"` using `present::status_bucket_str`.
    ///
    /// Projects present in `prev` but gone from `next` produce nothing — a project dropping
    /// out of the scan is not an event the user did.
    ///
    /// The rows produced by one call are pushed oldest-first, so the newest of them ends up
    /// at the very front; within one timestamp the order is by project name then kind, so
    /// the result never depends on `Vec` iteration luck. The queue is truncated to
    /// `FEED_CAPACITY` from the back afterwards.
    pub fn ingest(&mut self, prev: &Radar, next: &Radar) {
        // Match the previous snapshot's projects by id.
        let mut prev_by_id: HashMap<&str, &Project> = HashMap::new();
        for p in &prev.projects {
            prev_by_id.insert(p.id.as_str(), p);
        }

        let mut rows: Vec<FeedEvent> = Vec::new();

        for p in &next.projects {
            let project_name = p.name.clone();
            let prev_p = match prev_by_id.get(p.id.as_str()) {
                Some(p) => p,
                None => {
                    rows.push(FeedEvent {
                        at: next.updated_at,
                        project: project_name.clone(),
                        kind: FeedKind::Appeared,
                        detail: "discovered".to_string(),
                    });
                    continue;
                }
            };

            // Agent: a strictly newer last_event_at. `None` counts as older than any `Some`.
            if let Some(new_at) = p.agent.last_event_at {
                match prev_p.agent.last_event_at {
                    None => rows.push(FeedEvent {
                        at: new_at,
                        project: project_name.clone(),
                        kind: FeedKind::Agent,
                        detail: agent_detail(p),
                    }),
                    Some(old_at) => {
                        if new_at > old_at {
                            rows.push(FeedEvent {
                                at: new_at,
                                project: project_name.clone(),
                                kind: FeedKind::Agent,
                                detail: agent_detail(p),
                            });
                        }
                    }
                }
            }

            // Commit: a strictly newer last_commit_at. `None` counts as older than any
            // `Some` here exactly as it does for the agent arm above — a repo landing its
            // first-ever commit while petri is open is a commit, not silence.
            if let Some(new_commit) = p.git.last_commit_at
                && prev_p
                    .git
                    .last_commit_at
                    .is_none_or(|old_commit| new_commit > old_commit)
            {
                let branch = p.git.branch.clone().unwrap_or("detached".to_string());
                rows.push(FeedEvent {
                    at: new_commit,
                    project: project_name.clone(),
                    kind: FeedKind::Commit,
                    detail: format!("commit on {branch}"),
                });
            }

            // `MECH-5`: a *new* waiting latch. Keyed on the timestamp changing rather than
            // on `is_some()`, because the latch is carried forward unchanged across every
            // tick it stays live — an `is_some()` test would re-emit the same row on every
            // scan for as long as the human took to answer, which is precisely the period
            // the feed is most likely to be read.
            if let Some(since) = p.agent.waiting_since
                && prev_p.agent.waiting_since != Some(since)
            {
                rows.push(FeedEvent {
                    at: since,
                    project: project_name.clone(),
                    kind: FeedKind::Waiting,
                    detail: "▲ waiting on you".to_string(),
                });
            }

            // Bucket: a section move.
            if p.status_bucket != prev_p.status_bucket {
                rows.push(FeedEvent {
                    at: next.updated_at,
                    project: project_name.clone(),
                    kind: FeedKind::Bucket,
                    detail: format!(
                        "{} → {}",
                        status_bucket_str(&prev_p.status_bucket),
                        status_bucket_str(&p.status_bucket)
                    ),
                });
            }
        }

        // Oldest first (at, then project, then kind); pushing oldest-first to the front
        // leaves the newest at the very front, deterministically.
        rows.sort_by(|a, b| {
            a.at.cmp(&b.at)
                .then_with(|| a.project.cmp(&b.project))
                .then_with(|| a.kind.cmp(&b.kind))
        });
        for row in rows {
            self.events.push_front(row);
        }
        while self.events.len() > FEED_CAPACITY {
            self.events.pop_back();
        }
    }

    /// Newest-first view of the retained rows.
    pub fn events(&self) -> &VecDeque<FeedEvent> {
        &self.events
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// The colour a row is drawn in, by what produced it. Agent rows are the ordinary case and
/// stay in the foreground colour; the rarer structural events are tinted so they stand out
/// of a wall of agent chatter without needing a second column.
/// The colour of a row's time field, by whether it is a clock or a date.
///
/// Same-day-ness is a *recency* statement, so this reuses `theme`'s documented
/// `FRESH`/`COLD` silence gradient rather than inventing a pair for the feed alone — the
/// module doc there asks for exactly that ("one gradient, reused, rather than a flat color
/// that says nothing about what it's labeling").
///
/// A colour rather than a separator row is deliberate: the feed's row budget is scarce
/// (`FEED_MIN_ROWS` is 4, so as few as two body rows), and a divider would spend a whole
/// row of real activity on a boundary. Tinting the field costs no rows at all, and because
/// rows are strictly newest-first the transition still reads as a single clean break down
/// the block.
fn stamp_color(is_today: bool) -> Color {
    if is_today { theme::FRESH } else { theme::COLD }
}

/// The colour a row's *body* is drawn in, by what produced it. Independent of
/// `stamp_color`: the two live in different columns and answer different questions —
/// "when was this" versus "what was it".
fn kind_color(kind: FeedKind) -> Color {
    match kind {
        FeedKind::Agent => theme::FG,
        FeedKind::Commit => theme::ACCENT,
        FeedKind::Bucket => theme::AGING,
        FeedKind::Appeared => crate::theme::DIMMER,
        FeedKind::Waiting => theme::DANGER,
    }
}

/// Render the feed as exactly `rows` lines, `width` columns wide. `now` decides which rows
/// are recent enough to show a clock rather than a date — see `FeedEvent::row_text`.
///
/// Layout, top to bottom: one ` ACTIVITY` label line, then the newest `rows - 1` events,
/// newest first, each `FeedEvent::row_text` prefixed with a space and **truncated — never
/// wrapped** — to `width`. A `Paragraph` without `Wrap` would silently drop the overflow
/// anyway; truncating on char boundaries makes it deliberate and keeps multi-byte project
/// names from panicking a byte slice.
///
/// **The block draws no rule of its own.** Whatever sits above it already ends in one —
/// every section closes with a light rule, collapsed or not — so drawing another produced
/// two identical dividers in consecutive rows. On the degenerate screens where nothing
/// precedes it, the header's own heavy rule is directly above instead.
///
/// Returns an empty `Vec` when `rows < 2` — a label with no room for a single event is not
/// worth the row, and `feed_rows_for` already refuses to hand out fewer than
/// `FEED_MIN_ROWS`, so this is a defensive floor rather than a reachable layout.
///
/// When the feed holds no events at all the body is a single dim line saying so, rather
/// than an unexplained empty box: on a fleet that has genuinely been quiet, "nothing has
/// happened" is the honest reading and the label alone does not say it.
pub fn feed_block_lines(
    feed: &FeedState,
    now: DateTime<Utc>,
    width: usize,
    rows: usize,
) -> Vec<Line<'static>> {
    // A labelled block needs a rule + a label to hold; below that there is no honest way to
    // draw it, and `feed_rows_for` never asks for fewer rows, so this is a defensive floor.
    if rows < 2 {
        return Vec::new();
    }

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(rows);

    // Clipped like every other line: at a degenerate width the label would otherwise be the
    // one row that overruns the pane. Caught by `the_stamp_survives_a_narrow_pane_before_the_
    // body_does`, which is the first test to check a width narrower than the label itself.
    lines.push(Line::from(Span::styled(
        " ACTIVITY".chars().take(width).collect::<String>(),
        Style::default()
            .fg(theme::COLD)
            .add_modifier(Modifier::BOLD),
    )));

    // The body is the newest `rows - 1` events, newest first. An empty feed has nothing to
    // list, so its body is a single dim line saying so rather than an unexplained empty box.
    if feed.is_empty() {
        lines.push(Line::from(Span::styled(
            "nothing to show".chars().take(width).collect::<String>(),
            Style::default().fg(theme::DIMMER),
        )));
        while lines.len() < rows {
            lines.push(Line::default());
        }
    } else {
        // The deque already holds the newest at the front, so taking from the front yields
        // the newest `rows - 2` events in newest-first order.
        let body: Vec<&FeedEvent> = feed.events().iter().take(rows - 1).collect();
        for e in body {
            // Two spans, because the stamp and the body answer different questions and are
            // coloured on different axes (recency vs kind). The width budget is spent on the
            // stamp first: it is the column that makes the block chronological, so it is the
            // last thing that should be lost to a narrow pane.
            //
            // Truncation is by DISPLAY COLUMNS throughout — never wrapping, never byte
            // slicing (which a multi-byte project name would panic on), and not by
            // character count either: `width` is a column budget from the layout, and a
            // project or branch name holding a wide character makes those two disagree,
            // which shows up as the row bleeding past its pane.
            let head = crate::width::take_width(&format!(" {}  ", e.stamp(now)), width);
            let remaining = width.saturating_sub(crate::width::width(&head));
            let body_text = crate::width::take_width(&e.body_text(), remaining);
            lines.push(Line::from(vec![
                Span::styled(head, Style::default().fg(stamp_color(e.is_today(now)))),
                Span::styled(body_text, Style::default().fg(kind_color(e.kind))),
            ]));
        }
        // Pad to the full height, exactly as the empty branch does. A feed holding fewer
        // events than the block has room for is the ordinary case on a freshly-started
        // petri, and the contract is `rows` lines either way.
        while lines.len() < rows {
            lines.push(Line::default());
        }
    }

    lines
}
