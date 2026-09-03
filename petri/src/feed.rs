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
    pub fn row_text(&self) -> String {
        format!("{}  {} · {}", self.at.format("%H:%M"), self.project, self.detail)
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
                && prev_p.git.last_commit_at.is_none_or(|old_commit| new_commit > old_commit)
            {
                let branch = p.git.branch.clone().unwrap_or("detached".to_string());
                rows.push(FeedEvent {
                    at: new_commit,
                    project: project_name.clone(),
                    kind: FeedKind::Commit,
                    detail: format!("commit on {branch}"),
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
fn kind_color(kind: FeedKind) -> Color {
    match kind {
        FeedKind::Agent => theme::FG,
        FeedKind::Commit => theme::ACCENT,
        FeedKind::Bucket => theme::AGING,
        FeedKind::Appeared => Color::DarkGray,
    }
}

/// Render the feed as exactly `rows` lines, `width` columns wide.
///
/// Layout, top to bottom: one light rule, one ` ACTIVITY` label line, then the newest
/// `rows - 2` events, newest first, each `FeedEvent::row_text` prefixed with a space and
/// **truncated — never wrapped** — to `width`. A `Paragraph` without `Wrap` would silently
/// drop the overflow anyway; truncating on char boundaries makes it deliberate and keeps
/// multi-byte project names from panicking a byte slice.
///
/// Returns an empty `Vec` when `rows < 3` — there is no honest way to draw a labelled block
/// in two lines, and `feed_rows_for` already refuses to hand out fewer than
/// `FEED_MIN_ROWS`, so this is a defensive floor rather than a reachable layout.
///
/// When the feed holds no events at all the body is a single dim line saying so, rather
/// than an unexplained empty box: on a fleet that has genuinely been quiet, "nothing has
/// happened" is the honest reading and the label alone does not say it.
pub fn feed_block_lines(feed: &FeedState, width: usize, rows: usize) -> Vec<Line<'static>> {
    todo!("SPACE-1 phase B")
}
