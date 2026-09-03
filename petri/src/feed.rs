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

use chrono::{DateTime, Utc};
use petridish_core::present::status_bucket_str;
use petridish_core::schema::{Project, Radar};
use std::collections::VecDeque;

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
        todo!("SPACE-1 phase A")
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
    todo!("SPACE-1 phase A")
}

/// The `detail` string for an agent event on `p`: `"{agent} {event}"`, plus a
/// `" · {n} file(s)"` suffix when the project is a dirty repo.
///
/// `active_agent` missing falls back to the literal `"agent"` (agent-agnostic — never a
/// specific vendor), `last_event` missing falls back to `humanize_event`'s `"activity"`.
/// The file count is singular at 1 (`"1 file"`), plural otherwise, and is omitted entirely
/// when the project is not a repo or has no uncommitted files.
pub fn agent_detail(p: &Project) -> String {
    todo!("SPACE-1 phase A")
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
        todo!("SPACE-1 phase A")
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
        todo!("SPACE-1 phase A")
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
