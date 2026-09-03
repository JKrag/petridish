# `petri` (Rust edition) — specification

**Status:** S0–S7 (§9's build order) are implemented and merged — real terminal, both
screens, filtering, collapsible sections, worktree nesting, `petri.toml` persistence.
Iterating on top of that (dashboard density/grid/sparkline work, palette unification) is
ongoing; this doc is the live spec for that code, not a pre-implementation plan. Supersedes
`ARCHITECTURE.md` §6, which described the Python/curses build.

`petri` is the interactive frontend of petridish: a terminal dashboard over
`~/.petridish/projects.json`. This document is the authoritative spec for the Rust
build. Terminology is defined in `CONTEXT.md` — **Dashboard**, **Browser**,
**petripy**, **state file** and **preferences file** are used here in exactly the
senses defined there.

A note on this doc's own history, since it's directly relevant to how much weight to put
on any one line here: earlier drafts of this spec were written before `petri` itself had
a line of Rust — back when the plan was still a curses port, no ratatui, no truecolor. A
few of that era's decisions (ANSI-16-only color, described in the old §4) got superseded
by later, in-code product decisions once real screens existed to look at, and this doc
lagged the code in saying so. Where that happened, the section below states the *current*
rule and folds the old one in as a "was X, changed because Y" note — it does not pretend
the old rule was never written, but it also doesn't require excavating git blame to find
out it no longer applies. If you're relying on a specific line here to justify not doing
something, and the code already does that something, trust the code and fix this doc.

---

## 1. What this is, and what it deliberately is not

It is a **reimplementation**, not a port. The behaviour of [[petripy]] is the parity
baseline — *the running code*, not any prose description of it. But petripy's
internals are shaped by curses, which offers `addstr` and nothing else; every screen
there is a pure function returning `list[str]` with the padding arithmetic done by
hand. ratatui does layout, constraint solving and widget composition properly.

So the split carried across is:

- **Kept:** the pure derivation layer — grouping, bucket membership, worktree
  rollup, quietest-first ordering, silence seconds, humanised durations, selection
  movement. These are real logic and they are testable without a terminal.
- **Discarded:** the string-layout layer. No `list[str]` screens, no manual column
  padding, no `_clip`/`_fit`/`_split` helpers. ratatui's `Layout` replaces them.

**Explicit non-goal: byte-for-byte equivalence with petripy.** There is no
differential oracle (contrast `swab/scripts/diff_check.sh`, which did exactly that
for the scanner port). Rationale and consequences: ADR-0003.

**Unchanged invariants.** `petri` never writes the state file — `swab scan` is its
only writer. `petri` owns exactly one file, the preferences file (§6).

---

## 2. Crate layout

A cargo workspace at the repo root. `swab/` stays where it is (it is referenced ~30
times across the docs, `install.sh`, `pyproject.toml` and its own `scripts/`) — the
workspace members do not need a common parent directory.

```
Cargo.toml            # [workspace] members = ["petridish-core", "swab", "petri"]
petridish-core/       # schema + presentation helpers, shared
swab/                 # scanner: bins swab, swab-hook   (writes the state file)
petri/                # TUI: bin petri                  (reads it)
src/petridish/        # Python read-side: petripy, menubar, installer
```

`petri` is its own crate, not a third `[[bin]]` in `swab`, because `swab-hook` is
the declared latency path and has no business with ratatui/crossterm anywhere in its
dependency tree. Full reasoning: ADR-0002.

### `petridish-core`

- `schema` — the serde types: `Radar`, `Project`, `GitState`, `AgentSignal`,
  `QuotaState`, `StatusBucket`, plus `read_json`. Moved out of `swab/src/schema.rs`;
  `swab` depends on core for them rather than owning them.
- `present` — the pure derivations both surfaces need:
  `status_bucket_str`, `agent_activity_str`, `agent_label`, `dirty_marker`,
  `worktree_parent_name`, `silence_seconds`, `humanize_duration`, `is_stale`.

`swab/src/cli.rs::_print_table` is refactored to call `present` rather than
open-coding the agent label, dirty marker and `name (in parent)` cell. Its existing
tests (`list_table_name_cell_shows_worktree_parent` and neighbours) are the safety
net for that refactor and must not be modified.

Column-width computation stays in `cli.rs`. It is CLI-specific — ratatui does its
own layout.

### `petri`

- `app.rs` — the S4 walking-skeleton entry point and its render function; superseded
  as the real screen once `browser.rs`/`dashboard.rs` landed, kept as the harness
  S4's own tests still exercise.
- `browser.rs` — the Browser screen (§3.1).
- `dashboard.rs` — the Dashboard screen (§3.2).
- `theme.rs` — the shared truecolor palette both screens draw from (§4.1).
- `prefs.rs` — the preferences file (§6).
- `lib.rs` — the event loop, terminal setup/teardown, key handling, `run()` entry point.

---

## 3. Screens

Two screens, `Tab` to switch, per `CONTEXT.md` § "Screens". Room for more later.

### 3.1 Browser (build first)

The dense tool you drive. Built before the Dashboard: it holds the harder
interaction machinery (selection, filtering, scrolling, detail pane), the Dashboard
reuses all of it, and the Dashboard's `Enter` handoff needs somewhere to land.

- Grouped list, sections in the fixed order `active, in_flight, stale, cold`, each
  with a header and a count. Section labels: `RUNNING` / `IN FLIGHT` / `STALE` /
  `COLD`.
- Excludes `is_foreign` projects. No `--all` toggle — that is `swab list --all`'s
  job.
- Row: agent glyph (`●` working / `○` otherwise), name, uncommitted-file count,
  silence age.
- **Detail pane on the right**, always right (never reflowed below — deferred, §7).
  If the window is too narrow to give it a usable width, hide it entirely rather
  than squeezing. Shows: path (`~`-abbreviated), branch, dirty file count, last
  commit time (plus `mine_last_commit_at` when it differs), github url, agent
  state / active agent / session id, `last_activity_at`. Renders a `nothing
  selected` state when the filtered set is empty.
- **Selection** moves by a delta, **crossing section boundaries** and skipping
  empty sections, **clamped — never wrapping** — at the top and bottom of the whole
  list. The empty selection must be representable and must not panic. Re-filtering
  must not panic when the previously-selected project is filtered out; selection
  resets to the first available row.
- **`/` opens a type-ahead filter** that live-filters as you type: case-insensitive
  substring match against `Project.name`; an empty query returns the input
  unchanged. `Enter`/`Esc` closes it, `Esc` clears, `Backspace` deletes the last
  character and re-filters (char-wise, so a multi-byte character goes as one
  keypress).
- **An active filter is always visible in the header** (`IDEAS.md` `ACT-10`), as a
  chip after the title badge: the query, and `<matched> of <total>` against the
  *unfiltered* row count. It is drawn bright with a block cursor while the input
  is open and dim without one once `Enter` has closed it — the second state is the
  one that matters, since a short list with no query on screen is indistinguishable
  from a fleet that has gone quiet, and `0 of 12` is what separates "your query
  excluded everything" from "there is nothing here". It is a header chip rather
  than in the footer because the footer's keymap stays useful *while* you filter
  — swapping it out for a filter prompt would trade a permanently-useful surface
  for a transient one, and the header had the room. The header row cannot wrap,
  so on a narrow terminal the **count keeps its width and the query is elided**
  (`…`) into what is left — while the input is open the tail is kept so further
  keystrokes stay visible, once it is closed the head is kept so the query stays
  identifiable.
- **Scrolls**, with a scrollbar. "Every project must be reachable" is this screen's
  rule; truncating it would break that. Contrast the Dashboard (§3.2).
- Footer keymap. Keep it honest and useful (see §5) — it is not a fixed contract.
- **Selection highlight and section-label colors follow the shared palette (§4.1)**:
  selection renders as a solid reverse-video bar (black text on `theme::ACCENT`,
  bold) — the same convention `dashboard.rs`'s `solid_selected_line` uses for its
  compact rows — not a bare text-color change, since a hue shift against similarly
  light text reads as a much weaker focus signal than an actual filled bar. Section
  headers use `theme::bucket_color` (RUNNING fresh-green, IN FLIGHT amber,
  STALE/COLD grey), the same function the Dashboard's headers use, so the two
  screens' headers read as one vocabulary.

### 3.2 Dashboard

The ambient monitor: "does anything need me?" across a fleet of unattended runs.

- Header: `petri · dashboard`, project count, clock, last scan duration.
- **`RUNNING`** — membership per ADR-0001: a project counts as running if its own
  bucket is `active` *or* it has an active worktree child. Ordered **quietest
  first** (longest-silent at the top — the stalled run is the one that needs you)
  **within a 3-hour attention ceiling** (`RUNNING_ATTENTION_CEILING_S`); past that
  ceiling, silence has stopped meaning "might be stalled" and started meaning
  "probably forgotten," so those projects sort as one group below everything still
  under the ceiling rather than competing with it on raw duration — unbounded
  quietest-first let a days-old forgotten tab permanently bury a session someone was
  actively mid-run on, the opposite of the point of this ordering. They stay in
  RUNNING either way (a live agent process in a forgotten tab still counts, ADR-0001)
  — just not at the top. Label degrades to `RECENT` when nothing in the section has
  an agent at all, because `RUNNING` would then overstate it.
  - **Density is row-budget-driven, not width-driven**: above
    `COMPACT_TIER_MAX_CONTENT_ROWS` (16) content rows, RUNNING renders roomy
    bordered cards (glyph, name, overall silence in the header; a `git` zone row —
    branch, dirty marker, commit age, git-activity sparkline — and an `agent` zone
    row — agent name, session id, agent-activity sparkline — each pairing its own
    facts with its own sparkline rather than two look-alike bars stacked with no
    visual link to either one's data; `~`-abbreviated path). Below that ceiling it
    drops to the same single-line compact row IN FLIGHT/STALE/COLD already use —
    the real-world case is a narrow split pane wide enough for a roomy card's
    fields but too short to show more than a handful of them, so density responds
    to vertical room, not horizontal.
  - **Worktree nesting:** an active worktree indents under its parent when the
    parent is also in this section; when it is not, it falls back to the
    `name (in parent-name)` suffix form `swab list` uses. Display-only — the
    `status_bucket` in the state file is never touched (ADR-0001).
- **`IN FLIGHT` / `STALE` / `COLD`** — compact rows: name, branch, `✎N`, commit
  age, `gh` marker. IN FLIGHT rows additionally carry their own git-activity
  sparkline (STALE/COLD omit it and keep the plain fields — the 14-day window is
  wide enough to be worth the space only where commits are still expected soon). A
  parent with worktree children in any bucket shows `name · N worktrees` on its own
  row rather than listing them.
- **Sections lay out as a responsive grid, not a single always-narrow column.**
  Column count is derived from width (`dashboard.rs`'s `grid_columns`/
  `plan_layout`, capped at `MAX_GRID_COLUMNS`); items are assigned row-major (left
  column, then right, then the next row down) so the selection cursor stays the
  flat sequence described below — `j`/`k` just hops card-to-card, wrapping row to
  row, with no change to `DashboardState`. The agent-activity sparkline's sample
  count scales with the card's own allocated width (more width shows more real
  history, up to the ring's own retention ceiling); the git-activity sparkline does
  not scale the same way, since there is no more daily-commit history to show than
  the fixed retention window already holds.
- **Sections are collapsible.** A collapsed section keeps its header and count and
  yields its rows' space to the sections left open. Defaults: `RUNNING` and
  `IN FLIGHT` expanded, `STALE` and `COLD` collapsed. This is how real estate gets
  allocated on this screen — deliberately by the user, not by a fixed priority
  ladder.
- **A *run* of consecutive collapsed sections shares one line**, tab-style:
  `IN FLIGHT 6  ·  STALE 32  ·  COLD 27`, bracketed by the same two light rules a single
  section header gets. Three collapsed sections therefore cost 3 rows in total, not 9 —
  collapsing used to reclaim a section's rows while still spending a full header's chrome
  on each, which on a "show me only what's running" layout gave back less than it looked
  like. Each entry keeps its own bucket colour and count and stays an independent
  selection stop, so `j`/`k` walks along the strip and the selected entry carries the
  usual highlight — collapsed sections remain actionable rather than becoming a summary
  you cannot reopen. Runs group **in place** rather than gathering every collapsed section
  into one strip, so a section never jumps out of `SECTION_ORDER` position. Entries that
  do not fit the width are dropped with a trailing `+N…`, never silently.
- **Section headers are selection stops, and this is load-bearing.** A collapsed
  section has no visible rows, so if the cursor only visited rows there would be no
  way to put the cursor on a collapsed section and therefore no way to reopen it —
  and `STALE`/`COLD` ship collapsed by default. So the Dashboard's cursor sequence
  is: `RUNNING` header, its rows, `IN FLIGHT` header, its rows, `STALE` header,
  (its rows only if expanded), `COLD` header, … Consequences, all of which need
  tests:
  - Selection never enters a collapsed section's contents — those rows are not
    rendered, so they are not stops.
  - A section with **zero** projects is not rendered at all and contributes no stop.
    Collapsed ≠ empty: a collapsed section with 27 projects *is* a stop.
  - With `Space` on a header: toggle that section. With `Space` on a row: toggle the
    section containing it, and selection moves to that section's header so the
    cursor is never left pointing at a row that no longer exists.
  - `Enter` on a header toggles as well (it is the obvious thing to press); `Enter`
    on a row jumps to the Browser.
  - The Browser has no collapsible sections in v1, so its headers are **not**
    selection stops — its selection visits rows only, matching petripy. The
    clamping and boundary-crossing rules in §3.1 are written for that simpler case
    and do not transfer unexamined.
- **Overflow: truncate, do not scroll.** If even the expanded sections exceed the
  height, sections emit in priority order and stop, showing `… +N more` where the
  cut fell. A glance surface that can be scrolled into a state where it hides the
  alert is a worse monitor. The `… +N more` marker is required — silent truncation
  is the failure mode this replaces.
- **Selection cursor.** `j`/`k`/arrows highlight the current card or row; `Enter`
  switches to the Browser with that project selected and its detail pane open.
  petripy made this screen strictly input-free so it could be a pure function; in
  real use that was the wrong trade (see `CONTEXT.md`). Inline expansion in place
  is a likely future addition, not v1.
- **Activity feed (SPACE-1).** Surplus height below the fleet carries a rolling
  record of what the fleet has been doing — a light rule, an ` ACTIVITY` label, then
  newest-first rows shaped `14:22  project-radar · claude-code Bash · 3 files`. Five
  things pin its behaviour:
  - **It is derived by diffing successive `Radar` snapshots, not by reading
    `events.ndjson`.** That file is truncated by `swab`'s scanner on every tick
    (`swab::events::read_and_compact` — events are consumed exactly once), so it holds
    at most one tick and a reader would race the truncation. `projects.json` carries
    the same facts durably. Consequence, stated rather than hidden: the feed advances
    at **scan cadence**, not in real time, and nothing about it may be labelled "live".
  - **It always yields to project rows.** Drawn only when no section was skipped and
    none truncated. This is not belt-and-braces: `plan_layout` really can leave
    surplus *while* truncating, because a roomy card spans 7 rows — a 20-row budget
    fits two of them and strands five. Spending those on a feed while a `… +N more`
    marker is on screen would inverse this section's whole priority order.
  - **Suppressed in the compact tier**, and otherwise **grows to fill the slack** — that
    is SPACE-1's whole purpose. There is no fixed ceiling; the only bound is how much
    activity there is to show (`events + 2` for the rule and label), because surplus the
    feed would fill with blank rows belongs back in the layout, not fenced off inside a
    mostly-empty block. An earlier version capped it at 12 rows to reserve height for
    `SPACE-2`/`SPACE-3`; that reserved space for features that do not exist against the
    one that does, and left two thirds of a 30-row surplus blank.
  - **Rows carry a date, not a clock, once they are from an earlier day** (`09-02` in
    the same five columns), and the time field is **tinted on that axis**: `FRESH` for
    today's clocks, `COLD` for earlier dates, reusing §4.1's existing silence gradient
    rather than a pair invented for this block. A bare clock renders yesterday's `23:14`
    below today's `04:58` and reads as a sorting bug; `MM-DD` alone fixes the ambiguity
    but is too weak a cue to catch while glancing, which is the only way this block is
    read. A colour rather than a separator row because the feed's row budget is scarce
    (as few as two body rows) — a divider would spend real activity on a boundary.
    The row's *body* keeps its own colour, by event kind: the two columns answer
    different questions ("when was this" vs "what was it") and vary independently.
  - **The event name comes from the transcript, not from the hook** — `swab`'s
    `sensors/claude.rs::event_name_for`, so this bullet describes a dependency, not
    `petri` code. It was once true that `agent.last_event` was `null` for *every*
    project in a real `projects.json`, which made `agent_detail`'s `"{agent} activity"`
    fallback the only thing anyone ever saw; that is fixed at the source. What `petri`
    must keep is the fallback itself, which is load-bearing rather than vestigial: the
    sensor derives names from an **allowlist** of conversational record types, so an
    unmodelled or future record type yields `None` on purpose and lands back on
    `"activity"`. A row reading `"claude-code activity"` is therefore honest output, not
    a bug to chase. Expect `"{agent} {tool}"` (`Bash`, `Edit`) from a project mid-run,
    and the blunter `"assistant message"`/`"user prompt"` from a dormant one whose
    transcript ended on a reply — the feed is sharpest exactly where it is being watched.
- **Staleness banner:** when the state file's `updated_at` is older than 24h
  (matching `swab doctor`'s freshness check), render normally *and* show a
  persistent banner (`▲ Data stale (updated {age} ago)`, on the danger color, §4.1)
  — `▲`, deliberately, not `⚠`: the latter is the exact codepoint §4.2's founding
  incident is about, and this is precisely the banner that must never fail
  silently. The screen must degrade visibly, never silently lie about freshness.

---

## 4. Application shell

### 4.1 Palette

`petri` uses a fixed truecolor palette (`petri/src/theme.rs`), shared by both
screens: `theme::ACCENT` is the app's own identity color (badges, selection bars,
heavy chrome rules); `theme::FRESH`/`AGING`/`COLD` are a green→amber→grey silence
gradient applied everywhere something needs to preview how stale it is — a project
row, an agent glyph, a section label (`theme::tier_color`, `theme::bucket_color`);
`theme::FG`/`DIM`/`DIMMER`/`BRANCH`/`DANGER` round out foreground text, meta text,
receded structural chrome, branch names, and the dirty/danger signal respectively.
One palette, not two that happen to coexist — extend this module for a new screen,
don't invent a second one next to it.

**This was not the original rule.** This spec's first draft mandated ratatui's
ANSI-16 color names ("inherit the user's themed terminal") — a call made while
`petri` itself was still the unstarted plan for a Python/curses port, with no real
screen to look at yet. It was superseded by an explicit product decision, made once
ratatui was actually in use and a mockup existed to compare against, to go
truecolor for a distinct, screenshot-worthy identity (`dashboard.rs` adopted it
first; `browser.rs`, which had shipped against the original ANSI-16 rule, was
brought onto the same palette afterward so the two screens wouldn't visibly
disagree). If you're about to reach for `Color::Cyan`/`Color::Yellow`/etc. in a new
render path, don't — that's the superseded rule; use `theme::` instead.

This is a product/aesthetic decision, independent of §4.2's glyph constraint, which
has nothing to do with color and is not relaxed by anything here.

### 4.2 Glyph portability

**Every non-ASCII character `petri` puts on screen needs a documented, deliberate
reason — this is a tested constraint, not a preference.** Enforced by
`petri/tests/glyph_portability.rs`, which scans the production code of the three
modules that actually build on-screen content (`app.rs`, `browser.rs`,
`dashboard.rs`) against a hand-maintained, reasoned allowlist, and separately
verifies every allowlist entry really is `UnicodeWidthChar::width() == Some(1)` — a
single, unambiguous narrow cell — per the `unicode-width` crate ratatui and
crossterm actually use for their own cell math. A character that's `None` (needs a
combining base/selector) or `Some(2)` (genuinely wide) would misalign every column
after it; either is a straightforward reject.

**Why a gate exists at all, and why it isn't the same gate `petripy` used.**
petripy shipped `⚠` (U+26A0, Unicode 4.0) as its stalled-run glyph, and it rendered
as a **blank cell** on the macOS 14 CI runner: ncurses asks libc's `wcwidth()`
before placing a character, macOS's tables lag the standard, and an unrecognized
codepoint becomes a space. Nothing in a 300-test suite noticed — the stalled-run
glyph is, by the dashboard's own design, the row you opened it to find, and it
silently vanished while every substring assertion stayed green because the
project's name also appears on its own path row. Full account, kept in one place
rather than retold in three: `src/petridish/CLAUDE.md`'s "The `wcwidth` incident".

`petri` never calls `wcwidth` — that specific failure mode isn't known to reproduce
on this rendering path. What survives from the incident is the *lesson*, not the
specific bar: an unverified glyph can fail silently, in exactly the row a
monitoring dashboard exists to make visible, with every test still green. So the
gate is ported — a deliberate, reasoned allowlist beats "it looked fine on my
laptop" — but the *criterion* is grounded in what `petri` actually depends on
(`unicode-width`'s computed cell width) rather than in a proxy ("must predate
Unicode 1.1") that had no bearing on this dependency in the first place and would
have blocked glyphs that are simply fine here — Braille patterns (U+2800–28FF,
Unicode 3.0, the sparkline/graph glyph set tools like btop use) are a single narrow
cell by this measure and are not excluded by anything in this section.

**Known, deliberate scope limit:** the gate checks `width()`, ratatui's default
(non-CJK) interpretation, not `width_cjk()`. A few glyphs already in real use here
(`●` U+25CF, `█` U+2588) are East Asian Ambiguous — double-width under a terminal
explicitly configured for CJK ambiguous-wide handling, single-width everywhere
else. `petri` isn't targeting CJK-locale terminals as a first-class case today;
if that changes, this gate needs an explicit second CJK-mode pass, not a silent
tightening that starts failing on glyphs nobody re-evaluated.

### 4.3 Auto-poll

`stat` the state file's mtime on a short timer (2–5s) and re-read + re-render only
when it changed. A plain stat-poll, not a file watcher — not to avoid a dependency
(§10 is explicit that `petri` may take Rust dependencies freely; a watcher crate
would be a perfectly normal one), but because a poll is simpler to reason about and
test deterministically (no watcher-specific debounce/coalescing edge cases, no
platform-specific backend to verify) for a file that's rewritten at most once every
few seconds by `swab scan`'s own cadence — there's no latency budget here a watcher
would meaningfully improve on.

**A reload re-derives the screens but must not re-derive the *user*.** Which sections
are collapsed and which row is selected belong to the person, not to the scan, so
`DashboardState::refresh` carries both across a reload rather than rebuilding from
defaults. This is not a nicety: `swab` rewrites the state file every few seconds on an
active machine, so a reload that resets them makes the screen rearrange itself with no
input from the user — a section they collapsed reopens, and the cursor jumps to the top
mid-navigation. Both were live bugs, reported from real use. The cursor anchors on the
selected project's **name**, never its index into `radar.projects`: the scanner re-sorts
on every tick, so an index-based restore silently lands the cursor on a different
project, which is worse than losing it. `petri/tests/s10_pty_reload.rs` is the gate, and
it has to be a PTY test — the defect was at the poll loop's call site, so every
pure-state test passed while it was live.

### 4.4 Missing state file

Checked *before* entering the alternate screen. Print the same message `swab
list`/`swab path` use — `no state file at {path}; run 'swab scan' first` — and exit
1. Never a blank or broken screen.

### 4.5 Resize and tiny terminals

Must not panic. Clip, or show a `resize terminal` message. This includes the
degenerate 0×0 a freshly-forked pty reports.

### 4.6 Schema drift

`#[serde(default)]` on every field that can be absent, so a state file written by
an older `swab` still parses. If `schema_version` is *greater* than the version
this build knows, render normally but show a banner in the same slot as the
staleness banner. Never hard-fail on a readable file.

### 4.7 Terminal restoration

On every exit path, including panic — install a panic hook that leaves the
alternate screen and disables raw mode before unwinding. A panic that leaves the
user's terminal in raw mode is a v1 blocker, not a polish item.

---

## 5. Keybindings (v1)

| Key | Action |
|---|---|
| `Tab` | switch Dashboard ↔ Browser |
| `j` / `k` / `↑` / `↓` | move selection (both screens) |
| `J` / `K` | Browser: fast jump, ~10 rows |
| `PageDown` / `PageUp` | Browser: jump one screenful |
| `Home` / `End` | Browser: jump to the first/last row |
| `Enter` | Dashboard: on a row, jump to Browser on this project; on a section header, toggle it. Browser: **unbound** (see below) |
| `/` | Browser: open type-ahead filter |
| `Backspace` | Browser: delete the last character of the filter query |
| `Esc` | close/clear the filter |
| `Space` | Dashboard: collapse/expand the current section |
| `o` / `g` / `e` | Browser: open remote / git history / open in editor (§5.1) |
| `O` / `G` / `E` | Browser: re-pick the tool for that action (§5.1) |
| `q` | quit |

**`J`/`K`/`PageUp`/`PageDown`/`Home`/`End` are Browser-only, not Dashboard.**
The Dashboard's overflow model is "truncate, never scroll" (§3.2) — there is no
viewport to page through, and a big jump could land the cursor on a row that was
truncated out of the render entirely with no visual feedback. The Browser is the
one screen with a real scrolling viewport, so that's where fast navigation earns
its keep.

**The footer is a design problem, not a contract.** An earlier version of this
document made "advertise only keys that are actually bound" a hard requirement,
and it has been retired: it was never a considered decision, and treating it as a
gate pushed later work into shapes chosen to satisfy the rule rather than the
user. What survives is the sensible half — *don't advertise a key that does
nothing*, because a footer you can't trust is worse than no footer. Everything
else is judgement:

- It need not list *every* bound key. `PageUp`/`PageDown`/`Home`/`End` are bound
  and work but are absent from the Browser's footer, so the action keys fit on one
  line. That is a good trade, not an exception to a rule.
- It may change with mode, if a mode's keys are genuinely different and the swap
  reads as clearer rather than as the keymap vanishing.
- It may be machine-dependent, if that is honestly better. Today it isn't: the
  action keys are advertised unconditionally because `g` always resolves (§5.1's
  fallback chain) and `o`/`e` always respond, and because a machine-dependent
  footer would make every snapshot test depend on what happens to be installed on
  the machine running it.

The bar is "would a new user be misled?", not "does it match a list".

**`Enter` is deliberately unbound in the Browser's normal mode, and must not appear in
its footer.** The Dashboard's `Enter` exists to cure a real itch ("my fingers wanted to
arrow down to *something*"); the Browser is where you land. It *is* bound inside the
`/` filter (confirm) and inside the tool picker (choose), both of which are modes with
an obvious target — the rule is about the bare row cursor.

### 5.1 Action keys

**This supersedes an earlier version of this section**, which said "no launch,
open-editor or resume-session actions in v1 — `petri` staying read-only is
intentional." Actions arrived in slice 1 (`IDEAS.md` `ACT-1`/`ACT-2`/`MECH-2`), and
the read-only claim it was protecting is unchanged and still true in the only sense
that matters: **`petri` still never writes `projects.json`** (invariant 1). Handing the
terminal to `serie` is not the same thing as becoming a writer.

Actions are data, not hardcoded keys — `petri/src/tools.rs`'s registry. Each carries an
id, a key, candidate programs in preference order, an exec mode (`Terminal` suspends
`petri` and waits; `Background` spawns detached), and what the action needs *from the
project*. Availability therefore has two independent axes: whether any candidate is
installed on this machine, and whether this project has the target at all (a project
with no `github_url` leaves `o` nothing to open). Both degrade to a one-line notice,
never a crash.

Three actions are bound today: `o` (open remote), `g` (git history, which always
resolves thanks to a pinned `git log --graph` fallback) and `e` (open in editor).

**The shifted variant of any action key re-picks its tool** (`IDEAS.md` `ACT-11`),
opening the picker for a *one-off* launch: `Enter` runs the highlighted tool once and
leaves the stored default alone, `D` makes it the new default and runs it, `Esc` does
neither. The shifted key is derived from the action's own key, so a new registry entry
gets one automatically.

`Shift`+`Enter` is **not** available for this or anything else. Terminals do not report
it distinguishably from plain `Enter` without the kitty keyboard protocol, and `petri`
pushes no `KeyboardEnhancementFlags` — a binding on it would compile, test green, and
never fire. Do not add the flags to win one binding; it changes key decoding globally
and destabilises the PTY test layer (§8).

---

## 6. Preferences file

`~/.petridish/petri.toml`, owned and written by `petri` alone. Holds which Dashboard
sections are collapsed, the last screen, and a `[tools]` table mapping each action id
to the program the user chose for it (§5.1). A one-off launch (`Enter` in the re-pick
picker) deliberately does **not** write that table — writing it would cost the user the
very default they pressed the shifted key to bypass. `swab` never reads it; it is
deliberately *not* a `[petri]` section in `config.toml`, which would put two writers
on one file.

Written atomically (temp file + rename), same as `swab` does for the state file. A
missing file means defaults. A **corrupt or unparseable file means defaults plus a
warning** — never a crash, and never a refusal to start. There is a test for this.

---

## 7. Deferred (decided, not forgotten)

Cut from v1 by explicit decision:

- **Quota bars** (5h/7d percentage + reset countdown). Real daily value — this is
  the most likely first post-v1 addition.
- **COLD as a `·`-joined one-line name list.** A collapsed COLD section plus the
  Browser covers it; the joined line is a curses-era space-saving trick.
- **Responsive detail pane** (beside vs below). Always right, hidden when too
  narrow.
- **Inline card expansion** on the Dashboard.
- **A non-interactive `petri dash`** printing one frame and exiting (pipeable into a
  tmux status pane). Free while screens were pure functions; now needs an explicit
  off-screen buffer render.

**Not deferred, despite an earlier draft of this doc saying otherwise:** a
width-driven density switch between roomy and compact RUNNING rendering. That
earlier note assumed collapsible sections fully replaced the need for it; in
practice a real narrow-but-tall split pane still needed a row-budget-driven switch
independent of collapsing, which is what §3.2's `COMPACT_TIER_MAX_CONTENT_ROWS`
does now.

---

## 8. Verification

Four layers. Full reasoning: ADR-0003. This work is intended for unattended
(`delegate-afk`) execution, so the bar is "the machine can tell done from compiles".

1. **Pure-state unit tests.** Port the *test cases* — not the code — from
   `tests/test_tui_state.py` and `tests/test_screens.py`: grouping, bucket
   membership, worktree rollup, quietest-first ordering, silence seconds, humanised
   durations, selection movement across section boundaries, clamping at both ends,
   the empty selection, and re-filtering out the selected row.
2. **`TestBackend` buffer snapshots — structural, not exact goldens.** Render each
   whole screen at a few representative geometries (narrow/normal/wide — see each
   `sN_snapshot.rs`'s own choices) and assert on content that must appear (every
   visible project's name, a section header's text, the detail pane's presence or
   absence at a given width) rather than pinning the full rendered buffer
   byte-for-byte. Exact-buffer goldens were considered and dropped: they pin
   incidental layout (column widths, exact padding) as tightly as the properties
   that actually matter, so a legitimate layout tweak forces a snapshot
   re-record on every change instead of only failing when a real behavior breaks.
   These are self-referential goldens either way, *not* comparisons against
   petripy.
3. **PTY end-to-end tests** against the real binary, following
   `tests/test_tui_pty.py`'s hard-won lessons: keep draining the pty while waiting
   or the child blocks in `write()` and looks like it ignored your keystroke; set
   the winsize explicitly or a forked pty starts at 0×0. Cover: it starts, `Tab`
   switches, `/` filters, `j`/`k` move, resize survives, `q` returns the shell and
   the terminal is not left in raw mode.
   **These must be deterministic by construction**, because this layer is
   empirically the flakiest one in the repo — two of the Python equivalents pass
   locally and failed on the macOS CI runner, and this Rust layer has shown the
   same character under parallel test execution (see `pty_support/mod.rs`'s module
   doc comment for the specific races found and mitigated). Requirements:
   - **Assert against a settled full-screen snapshot, not the raw byte stream.**
     This is the big one. The captured failing CI frame was
     `petri · dashboard … ════ … ──── … tab browser z density q quit` — the whole
     RUNNING section simply absent, i.e. a partially-painted frame. Reading the pty
     byte stream also means "lines" are stream segments, not screen rows, which is
     what broke the other assertion. Read until the stream goes quiet, then assert
     on the reconstructed screen.
   - **Inject a fixed clock.** The `▲` stalled-run glyph is derived from silence at
     *render* time on purpose (`glyph_for` deliberately bypasses the stored
     `agent.state` so the glyph can't disagree with the live silence counter next
     to it), and the Python fixture builds its offsets from `datetime.now()`. Any
     wall-clock-derived assertion is a latent flake.
   - Set the winsize explicitly (a forked pty starts at 0×0) and pin `LANG`/`LC_ALL`
     to a UTF-8 locale.
   - Run this layer's tests single-threaded (`--test-threads=1`) if they're flaking
     under parallel execution in a constrained/sandboxed environment — this has
     been observed to be purely a scheduling/timing artifact of the sandbox, not a
     code regression (confirmed by the same suite passing cleanly single-threaded).

   A flaky layer is worse than no layer when nobody is watching: an unattended
   agent cannot tell a flake from a defect, so it either halts on a false failure
   or learns to ignore the layer.
4. **Human smoke test** — but as confirmation, not as the gate. "It works, and I
   have an idea for a change" is the expected shape of it.

### Fixtures

Four committed fixtures under `tests/fixtures/`, replacing the original
single-project `projects.golden.json` (kept alongside them for whatever Python-side
tests still reference it):

| File | Shape |
|---|---|
| `minimal.json` | one project |
| `normal.json` | ~15 projects, mixed buckets |
| `loaded.json` | ~70 projects, worktrees, every bucket populated |
| `hostile.json` | empty project list · all-cold · non-repo · null branch · 200-char name · CJK/emoji name · absent quota · `updated_at` 3 days old · worktree whose parent is absent · `schema_version` from the future |

`hostile.json` is where an unattended agent's bugs actually surface. Every snapshot
and pure-state test runs against it.

### Gate

`make check` includes `cargo test --workspace`, and `ci.yml` includes a Rust job.
`swab` was never in CI before this work; that gap is closed. An unattended agent
needs one command that is the entire truth.

---

## 9. Build order

Each slice is independently verifiable and committable — one commit per verified
slice, matching `delegate-afk`'s keep/revert checkpoint boundary. All slices below
are done; the table is kept (not collapsed into prose) because `petri/tests/s4_pty.rs`
through `s7_prefs.rs` are the slice names as literal file names — this is a live
index into the test suite, not a historical record.

| # | Slice | Verified by | Status |
|---|---|---|---|
| S0 | Rename the Python `petri` console script to `petripy`; repoint `install.sh`'s venv anchor from `petri` to `petridish-installer`; deprecation note in `README.md` | `pytest`, `install.sh` runs | done |
| S1 | Workspace refactor; extract `petridish-core` (schema out of `swab`); `swab` behaviour unchanged. `make check` + `ci.yml` gain `cargo test --workspace`. Repoint the doc references to `swab/src/schema.rs` (in `CLAUDE.md`, `CONTEXT.md`, `src/petridish/schema.py`) at their new home | existing `swab` tests green, unmodified | done |
| S2 | Fixture corpus (§8) | consumed by S3+ | done |
| S3 | `present` helpers into core; `cli.rs` refactored to call them | existing `cli.rs` tests green, unmodified | done |
| S4 | **`petri` walking skeleton**: real terminal, reads state file, header + flat list, `q` quits with the terminal restored, mtime poll, missing-file → exit 1, resize-safe, panic hook | first `TestBackend` snapshots + first PTY test | done |
| S5 | Browser: grouped list, selection across sections, detail pane, `/` filter, scrollbar, footer | snapshots ×3 geometries + PTY keystrokes | done |
| S6 | Dashboard: sections, roomy cards, quietest-first, worktree nesting, collapsible sections, cursor + `Enter`→Browser, staleness banner | snapshots + PTY | done |
| S7 | `Tab` switching + `petri.toml` persistence | snapshots + PTY + corrupt-toml test | done |

**S0 came first** so the Rust binary could take the `petri` name on `PATH` without
colliding, and petripy stayed runnable as a fallback throughout.

**S4 was deliberately trivial in content** — it carried all the infrastructure risk
(terminal setup, event loop, poll timer, panic hook, snapshot harness, PTY
harness), proven against a screen too simple to be wrong before S5/S6 built the
real screens on top of it.

Post-S7 work (dashboard grid layout, roomy-card zone rows, sparklines, palette
unification — see git log for the running list) continues under this same
one-commit-per-verified-change discipline; it isn't a numbered slice because it's
iteration on a finished v1, not a dependency chain the way S0→S7 was.

### `petripy`'s lifecycle

Moved to `src/petridish/CLAUDE.md` — that's petripy's own directory, and its
frozen/deprecated status plus deletion trigger belongs with the code it describes,
not duplicated here. Short version: frozen on arrival of the Rust build, still
installed and tested, not yet deleted.

---

## 10. Dependencies

`petri` may take dependencies freely; the stdlib-only rule was always
`src/petridish/`-specific and never applied to Rust. Pinned in `petri/Cargo.toml`:

- `ratatui` 0.30 (`TestBackend::assert_buffer_lines` is what layer 2 is built on)
- `crossterm` (matching ratatui 0.30's backend)
- `toml` for the preferences file (already a `swab` dependency)
- `chrono`, `serde`, `serde_json` (state-file parsing at runtime — see
  `petri/Cargo.toml`'s own comment for why `serde_json` is a direct dependency
  here rather than re-exported from `petridish-core`)
- `unicode-width`, dev-only (§4.2's glyph gate — pinned directly so the gate tests
  against the actual crate ratatui/crossterm depend on, not a copy of its behavior)
- `portable-pty`, dev-only (§8 layer 3's PTY harness)

Note for whoever next bumps ratatui: 0.30 split into `ratatui-core` /
`ratatui-widgets` / backend crates. Depend on the `ratatui` facade unless there is a
specific reason not to.
