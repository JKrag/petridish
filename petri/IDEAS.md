# `petri` — idea backlog

**Status: brainstorm, not spec.** Nothing here is decided. `petri/SPEC.md` is the
authoritative document for what `petri` *is*; this file is the pool of candidate
directions we draw from when deciding what it becomes next. An idea moving from here
into `SPEC.md` is the moment it stops being an idea.

Every entry has a stable ID (`ACT-3`, `SPACE-1`, …) so it can be referenced in
conversation, commits and issues without re-describing it — including from other docs
(`ARCHITECTURE.md` cites `SURF-3` by ID). **IDs are never reused or renumbered** — a
dropped idea keeps its ID and gets marked `[dropped]` with the reason.

**A built idea gets a one-line `DONE` pointer, not deleted** — deleting it would break
the cross-references open ideas make to it (`SURF-3` leans on `MECH-2`, `SURF-6` on
`MECH-1`, `ACT-6` on `ACT-1`). The full narrative — how it was built, the design mistakes
found along the way, the "findings worth keeping" — lives in `petri/IDEAS_LOG.md`, not
here. This file stays a pure backlog: what's still open, plus a pointer for what's
already built. (Before 2026-09-06 the two were one ~1000-line file; split because the
build history had made the still-open ideas hard to find.)

Origin: brainstorm session 2026-09-01, on the `petri-dashboard-redesign` branch, prompted
by two complaints about the then-current build — it wastes vertical space on tall
terminals, and the two screens are so close to each other that neither feels like it does
anything.

---

## Open ideas at a glance

| ID | one-liner |
|----|-----------|
| `MECH-4` | Embed a live child *inside* a ratatui pane — real but expensive, deprioritized |
| `ACT-2` | `f`/`y`/`s`/`?` (+ stretch `c`) → issue [#27](https://github.com/JKrag/petridish/issues/27); `t` → issue [#28](https://github.com/JKrag/petridish/issues/28) |
| `ACT-5` | Fleet-scoped actions on the Dashboard, not just project-scoped ones — needs more discussion before it's issue-ready |
| `ACT-6` | Multi-select and bulk action |
| `ACT-7` | Detail pane shows affordances, not just facts → issue [#32](https://github.com/JKrag/petridish/issues/32) |
| `ACT-9` | Target-availability should read as a dimmed affordance, not a transient notice |
| `SPACE-2` | Auto-expand STALE/COLD into leftover room |
| `SPACE-3` | A density tier above roomy, for very tall terminals → issue [#33](https://github.com/JKrag/petridish/issues/33) |
| `SPACE-4` | Make "truncate, don't scroll" height-conditional |
| `SURF-1` | A timeline/history screen across the fleet |
| `SURF-2` | Native notifications on state transitions — unblocked now that `MECH-5` exists, needs more discussion before it's issue-ready |
| `SURF-3` | Session manager for tmux/zellij → issue [#28](https://github.com/JKrag/petridish/issues/28) (merged with `ACT-2`'s `t`) |
| `SURF-4` | Quota/token pane, plus a key to a dedicated tool → issue [#29](https://github.com/JKrag/petridish/issues/29) |
| `SURF-5` | `petri dash --once` |
| `SURF-6` | A "project detail" popup on the Dashboard → issue [#30](https://github.com/JKrag/petridish/issues/30); design folded into `SURF-8` |
| `SURF-7` | A minimal `petri --mini` single-project screen → issue [#31](https://github.com/JKrag/petridish/issues/31); design folded into `SURF-8` |
| `SURF-8` | **The focus panel** — one renderer behind `SURF-6`/`SURF-7`/`ACT-7`, plus the rungs deferred out of its first pass |
| `SPACE-6` | An `icons` tier (unicode / Nerd Font) behind a `glyph::` indirection — buys horizontal room |
| `DATA-1` | git ahead/behind vs upstream |
| `DATA-2` | PR / CI state for the branch — network seam, argue on its own |
| `DATA-3` | stash count / unpushed-commit count |
| `DATA-4` | per-session token usage |
| `DATA-5` | per-project context-window percentage — the honest version of `ctx%` |

---

## 0. The framing these ideas came out of

Two observations shaped everything below.

**FRAME-1 — Most "make it interactive" ideas reduce to one primitive: suspend-and-exec.**
Open in editor, jump to a git graph, attach to tmux, open the GitHub URL — nearly all of
them are "hand the terminal to another program, take it back when it exits." Build that
once and the rest is a table of entries. See `MECH-2`.

**FRAME-2 — petri is a *router*, not a reimplementation of every tool it points at.**
Great TUIs already exist for git history (`serie`, `lazygit`, `gitui`, `tig`) and for
token usage. petri's unique asset is that it knows *which project needs you right now*;
its job is to notice that and hand you to the right tool for it, not to grow a mediocre
in-house version of each. This is also the sharpest one-line pitch for a public release:
not "another agent dashboard" but "the launcher for your fleet."

**Screen division of labour that follows from FRAME-2:**

- **Dashboard** — aggregation, glance, no scroll. Answers *"does anything need me?"*
- **Browser** — complete, scrollable, actionable. Answers *"show me everything, and let
  me act on it."*

The two screens feel redundant when the Browser has the *detail* half of that and none of
the *act* half. The lever is **actions in the Browser, aggregation on the Dashboard** —
not more metrics on both.

---

## 1. Mechanisms (the enabling primitives)

### MECH-1 — Popup / overlay frames
**DONE** (slice 1) — `petri/src/picker.rs`'s `render`, a centred popup over the Browser.
Customers: the `?` help popup, `MECH-1`-based confirmation dialogs, `SURF-6`. Full
reasoning: `IDEAS_LOG.md` slice 1.

### MECH-2 — Suspend-and-exec (hand the terminal to a child process)
**DONE** (slice 1) — `petri/src/exec.rs`. The three gotchas (`.status()` not `.output()`,
`Terminal::resize` not `Terminal::clear()` on the way back, restore-on-failure) are
documented as code comments in `exec.rs`, not here. Full reasoning: `IDEAS_LOG.md`
slice 1.

### MECH-3 — Spawn-and-detach (for GUI targets)
**DONE** (slice 1) — `petri/src/exec.rs`; `ExecMode::{Terminal,Background}` in
`tools.rs` is the per-entry mode this needed. Full reasoning: `IDEAS_LOG.md` slice 1.

### MECH-4 — Embedding a live child *inside* a ratatui pane
**Feasibility: real, but expensive — deprioritized for now.** `portable-pty` (already a
dev-dependency here) plus a vt-parsing widget such as `tui-term`. You become a
terminal-emulator host: resize propagation, key encoding, mouse forwarding, alternate-
screen semantics inside a sub-rect.

That cost is justified only for something you want *persistently alongside* the UI — a
live log tail in a corner — not for "press `g`, look at the graph, come back," which
`MECH-2` covers at a fraction of the complexity.

### MECH-5 — "Waiting on you" as a state the schema can express
**DONE** (slice 6) — `AgentState::waiting_since` (`petridish-core`),
`events::WaitingDeltas` + `scan::resolve_waiting`/`waiting_for_root` (`swab`), and
`dashboard::is_waiting` (`petri`). Unblocks `SURF-2`. Full reasoning: `IDEAS_LOG.md`
slice 6.

Left open, deliberately: the menubar (`menubar.py`) doesn't show waiting state, and the
end-to-end path hasn't been observed on a real blocked session yet.

---

## 2. Actions — making the Browser *do* something (`ACT-*`)

### ACT-1 — An external-tool registry, not hardcoded keys
**DONE** (slice 1) — `petri/src/tools.rs`. Narrow: `petri.toml`'s `[tools]` table
overrides only *which program* per action, not a full argv override — worth knowing
before extending it for `ACT-5`/`ACT-6`/`SURF-4`. Full reasoning: `IDEAS_LOG.md` slice 1.

### ACT-2 — Candidate action keys for the Browser
**PARTLY DONE** (slice 1 + 2 + #27) — seven of nine bound. `f`/`y`/`s`/`?` landed via
[#27](https://github.com/JKrag/petridish/issues/27); its stretch `c` was split off with
implementation ideas left as a comment on that issue rather than built, pending a
shell-wrapper design. `t` remains tracked as
[#28](https://github.com/JKrag/petridish/issues/28) (merged with `SURF-3` into a
broader multiplexer discussion).

| key | action | mechanism | status |
|-----|--------|-----------|--------|
| `o` | open the project's remote in a web browser | `MECH-3` — spawn-and-detach; `github_url` is already in the schema | **done** (`O` re-picks) |
| `g` | git history / graph | `MECH-2`, fallback chain — `ACT-3` | **done** (`G` re-picks) |
| `e` | open in editor | `MECH-2` or `MECH-3` depending on target — `ACT-4` | **done** (`E` re-picks) |
| `t` | attach to the agent's tmux session | `MECH-2` (`attach`), or `switch-client` when already inside tmux | [#28](https://github.com/JKrag/petridish/issues/28) |
| `f` | reveal in Finder | `MECH-3`, `open <path>` | **done** |
| `y` | yank path to clipboard | for pasting into another terminal; the one row that is *not* a registry entry, no child process involved | **done** |
| `c` | `cd` here on exit | print the path for a shell wrapper to consume; petri becomes a navigator | split off #27, not yet its own issue — needs a shell-wrapper story first |
| `s` | rescan now | invoke `swab scan` and refresh — `swab scan` otherwise only runs on `com.petridish.daemon.plist`'s 60s `StartInterval`, and hooks never trigger a full scan themselves | **done** |
| `?` | help popup | `MECH-1`'s second customer, pure content generated from `tools::registry()` | **done** |

### ACT-3 — Git history with a graceful fallback chain
**DONE** (slice 1) — `registry()`'s `gitlog` action: `serie` → `lazygit` → `gitui` →
`tig` → plain `git log --graph`, pager pinned to dodge `less -F`'s flash-and-exit. Full
reasoning: `IDEAS_LOG.md` slice 1.

### ACT-4 — "Open in editor" and the folder-vs-file problem
**DONE** (slice 1) — editor resolution order (`petri.toml` → `$VISUAL` → `$EDITOR` →
probe) in `begin_action`; folder-capable candidates in `registry()`'s `edit` action.
**Still unverified**: the `Background` (GUI editor) path has only ever been
unit-tested, never exercised against a real editor on a desktop. Full reasoning:
`IDEAS_LOG.md` slice 1.

### ACT-5 — Fleet-level actions on the Dashboard
The Dashboard gets actions too, but *different* ones — fleet-scoped, not project-scoped.
"Attach to the run that needs me" (`Enter` on the top RUNNING card goes straight to tmux,
skipping the Browser) is the archetype. This is a large part of what stops the two screens
feeling like duplicates.

### ACT-6 — Multi-select and bulk action
Mark several projects, then apply one action to all of them (rescan, run a command in
each). Depends on `ACT-1`; only interesting once single-target actions exist.

### ACT-7 — Detail pane shows affordances, not just facts
Tracked as issue [#32](https://github.com/JKrag/petridish/issues/32). Follows from
`FRAME-2`: if petri is a router, the Browser's detail pane should be mostly *"here is
what you can do with this project"* rather than a static fact sheet. This changes the
pane's whole design and should be decided before investing more in its current layout.

### ACT-8 — First-run tool picker popup
**DONE** (slice 1) — `petri/src/picker.rs` + `[tools]` in `prefs.rs`: pops a picker only
when a choice is genuinely ambiguous, persists the answer. Refined by `ACT-11`. Full
reasoning: `IDEAS_LOG.md` slice 1.

### ACT-9 — Action availability has two independent axes
**DONE** (slice 1, tool-availability axis) — `Resolution::{NoTool,NoTarget}` in
`tools.rs`. Full reasoning: `IDEAS_LOG.md` slice 1.

**Still open — the target-availability axis.** A project can lack a target (no
`github_url`, no tmux session) even though the tooling is fine. That currently reads as
a transient notice; it should read as a disabled/dimmed affordance instead, so `o`/`t`
visibly show "nothing to act on here" rather than silently doing nothing.

### ACT-10 — The `/` filter query is never shown on screen
**DONE** (slice 3) — `browser::filter_chip_spans`: the active `/` query shows in the
header, bright with a cursor while typing, dim once closed. Full reasoning:
`IDEAS_LOG.md` slice 3.

### ACT-11 — The re-pick key is a one-off launcher, not a settings dialog
**DONE** (slice 2) — `picker::Mode::{FirstRun,Repick}`: the shifted action key
(`G`/`O`/`E`/…) opens a picker where `Enter` launches once and `D` sets the new default.
Full reasoning: `IDEAS_LOG.md` slice 2.

---

## 3. Reclaiming vertical space (`SPACE-*`)

**Diagnosis first — this is not a bug, it is three current rules with nothing to
counterbalance them.** `COMPACT_TIER_MAX_CONTENT_ROWS` is a *floor* (go compact below 16
rows); "truncate, do not scroll" caps content from above; STALE/COLD ship collapsed.
Nothing in the design ever *grows* to consume surplus height, so on a tall terminal every
section emits its fixed content and the remainder is blank by construction.

### SPACE-1 — Fill the slack with a live fleet event feed
**DONE** (slice 4) — `petri/src/feed.rs` + `dashboard::feed_rows_for`: a rolling fleet
activity feed fills leftover Dashboard height, derived from successive `projects.json`
snapshots (not `events.ndjson` — that file is a hand-off buffer, truncated every tick;
see `IDEAS_LOG.md` slice 4). Full reasoning: `IDEAS_LOG.md` slice 4.

### SPACE-2 — Auto-expand STALE/COLD into leftover room
The collapse defaults were space rationing for small terminals, never a stated preference.
With 20 spare rows, spend them; re-collapse when the room goes away. Must not fight a
user's explicit toggle.

### SPACE-3 — A third density tier *above* roomy
Tracked as issue [#33](https://github.com/JKrag/petridish/issues/33). Symmetric to the
existing compact/roomy switch: on tall terminals, more sparkline history, more fields per
card, bigger cards.

### SPACE-4 — Make "truncate, do not scroll" height-conditional
The rule exists so a glance surface can't be scrolled into a state where it hides the
alert — a risk that is really about *small* terminals. Truncating with blank space below
is the rule failing on its own terms. Worth revisiting rather than treating as settled.

### SPACE-5 — Collapsed sections as a tab strip, not three rows each
**DONE** (slice 5) — `dashboard::collapsed_strip_line` + the `joins_open_strip` branch in
`plan_layout`: consecutive collapsed sections (STALE/COLD/…) share one 3-row strip
instead of 3 rows each; each entry stays a selectable stop. Full reasoning:
`IDEAS_LOG.md` slice 5.

---

## 4. New surfaces (`SURF-*`)

### SURF-1 — A timeline / history screen
`events.ndjson` again: what happened across the fleet today, as one horizontal swimlane
per project. Nobody else can show this, because nobody else is collecting fleet-wide agent
events.

### SURF-2 — Native notifications on state transitions
**Unblocked** — `MECH-5` (waiting-on-you) landed in slice 6, so the natural trigger now
exists: notify on entering `waiting`, not on crossing a silence threshold ("crossed a
silence threshold" is weak — silence is ambiguous between thinking, blocked and finished;
"waiting on you" is not). `osascript`, no new dependencies.

### SURF-3 — Session manager for tmux / zellij
Tracked as issue [#28](https://github.com/JKrag/petridish/issues/28), merged with
`ACT-2`'s `t`. Given projects and their sessions, "give me a session for this project,
creating it if it doesn't exist" is a small amount of shell-out and is genuinely
daily-useful — but which multiplexers to support (tmux, zellij, herdr, …) and the right
abstraction needs discussion before implementation.

### SURF-4 — Quota / token pane, plus a key to a dedicated tool
Tracked as issue [#29](https://github.com/JKrag/petridish/issues/29). `SPEC.md` §7
already lists quota bars as the most likely post-v1 addition. `FRAME-2` says: ship a
minimal in-house bar *and* bind a key to whichever token TUI the user prefers, via the
same registry.

### SURF-5 — `petri dash --once`
Also already deferred in `SPEC.md` §7: render one frame to an off-screen buffer, print,
exit — pipeable into a tmux status line. Cheap once the off-screen render exists, and a
good "look how it composes" line in a public announcement.

### SURF-6 — A "project detail" popup on the Dashboard
Design folded into `SURF-8`. Tracked as issue
[#30](https://github.com/JKrag/petridish/issues/30). Expand a card in
place to show bigger and better details, plus the action keys that are live for that
project. Show bigger sparklines or other metrics, and make the Dashboard feel like a
*dashboard* rather than a static list. Should allow the user to "focus" on a project
without leaving the Dashboard, and to act on it without going to the Browser.

### SURF-7 — A minimal "run" screen for a single project
Design folded into `SURF-8`. Tracked as issue
[#31](https://github.com/JKrag/petridish/issues/31). Basically a
`petri --mini` mode: one project, one screen, no list. Split your terminal, so you have a
small pane in the corner, run petri --mini, and have a live view of the run currently on
your screen, i.e. in this terminal window/tab. This is a natural extension of `SURF-5`
and `SURF-6`, and it is a good candidate for a demo GIF.

### SURF-8 — The focus panel: one renderer behind SURF-6, SURF-7 and ACT-7
Full design: `petri/PROPOSAL-focus-panel.md` (a proposal, not spec). One pure
`focus_lines()` rendering an ordered content ladder sized to whatever `Rect` it gets, mounted
three ways: a `MECH-1` popup over the Dashboard (`SURF-6`/#30), the whole screen
(`SURF-7`/#31), and the Browser's existing detail popup re-pointed at it (`ACT-7`/#32).
`SPACE-3`/#33's lush tier shares the ladder's *content ordering* and the per-field renderers,
deliberately **not** its layout function — a roomy card must stay uniform across a grid
column, a focus panel owns a full-width rect.

**Rungs deliberately deferred out of the first pass**, kept here so they are not lost:

- **The full `agent_activity` ring** (60 samples ≈ 1h; cards show ~20). The width-scaling
  sparkline already recovers most of this, and the oldest samples are the least legible part.
- **The full 14-day `daily_commits`** un-elided — the card already shows all 14 days.
- **The worktree family via `parent_path`** (ladder rung R8): siblings and their buckets,
  reconstructible today only by eye across the Browser list. Real value, but it needs a
  rollup design of its own, and issue #1's categorisation work will touch the same code.
- **`category`** — untouched by any surface today; belongs with #1, not here.

### SPACE-6 — An `icons` tier, behind a `glyph::` indirection
Filed under `SPACE-*` although it buys *horizontal* room rather than vertical — that is the
scarce axis in `SURF-7`'s corner pane: `` is one cell where `master` is six. The blocker is
not `SPEC.md` §4.2's allowlist (an entry is one line plus a reason, and the gate proves the
width automatically) — it is that **no terminal exposes "a Nerd Font is active" as a
queryable signal**, so there is nothing to detect. The whole ecosystem is opt-in: eza's
`--icons=always|automatic|never`, lazygit's version-pinned `gui.nerdFontsVersion` (v3 *moved*
the Material Design Icons codepoints), starship's three named presets.

Shape: an `icons` key in `petri.toml` (`"none"` default / `"unicode"` / `"nerdfont-v3"`) plus
a `glyphs` module so render sites ask for `glyph::branch()` / `glyph::agent(kind)` rather than
embedding codepoints — the same indirection `theme.rs` already gives the palette, and what
makes a per-agent glyph (claude-code vs copilot) possible at all.

The trap to design around: a Nerd Font codepoint sits in the Private Use Area, where
`unicode-width` reports `Some(1)`. It would **pass** the portability gate while rendering as
a blank on a machine without the font — exactly the silent-blank failure §4.2 exists to
prevent. Hence explicit opt-in: the user asserting they have the font is the only signal that
exists.

---

## 4b. Facts nobody collects yet (`DATA-*`)

A different axis from `MECH`/`ACT`/`SPACE`/`SURF`: those are things `petri` could *do* with
what it already has. These are facts simply not in `projects.json`, so each one is a `swab`
sensor change — a crate boundary and a scan-tick cost, not a frontend tweak.

### DATA-1 — git ahead/behind vs upstream
A `gix` revwalk against the tracking branch. No network, no new dependency, and it lands
straight in `SURF-8`'s otherwise-thin `repo` rung. The most valuable of this group.

### DATA-2 — PR / CI state for the branch
Nice to have, and it stretches the seam hardest: it would be the first thing making `swab
scan`'s latency depend on a network round-trip, on a 60s timer, per project. If built, it
probably wants its own cadence and its own cache rather than a slot in the scan tick.

### DATA-3 — Stash count, unpushed-commit count
Cheap once `DATA-1`'s revwalk exists.

### DATA-4 — Per-session token usage

### DATA-5 — Per-project context-window percentage
`QuotaState::context_used_pct` exists, but it is parsed from `~/.claude/last-status.json`'s
`context_window` — a single file owned by whichever session wrote it last. So today's value
is neither fleet-wide nor attributable to any particular project, and rendering it as
`ctx 13%` next to one project is a lie whenever two agents are running (the situation
petridish exists for). The honest version reads each transcript's own usage records and lands
on `AgentState`, next to `session_id` — not in a global header.

---

## 5. Constraints any of these must respect

Collected here because they are easy to trip over while building the above.

- **The footer must not mislead** (`SPEC.md` §5). It is a design problem, not a
  contract — it need not list every bound key, may change with mode, and may be
  machine-dependent if that is honestly better. What it may not do is advertise a key
  that does nothing: a key whose tool isn't installed should be resolved at the
  registry level (`ACT-1`/`ACT-3`) rather than by letting the footer lie.
- **The glyph gate does not see ratatui-generated border characters.**
  `tests/glyph_portability.rs` scans *string literals* in the listed render modules, but
  `picker.rs` and `help.rs` draw their popups with `Borders::ALL`, whose default glyph set is
  `┌ ┐ └ ┘ ─ │` — and `┌ ┐ └ ┘` are not in `ALLOWED` today. They are on screen, unreviewed,
  in exactly the way `feed.rs`'s `→` and `picker.rs`'s `▌` were. Same class of hole, found in
  review rather than by the gate. Worth its own issue: either add the four entries and note
  that widget-generated glyphs need manual registration, or teach the gate about the border
  sets the code actually asks for.
- **The PTY test layer is the flakiest layer in the repo** by `SPEC.md` §8's own account.
  A `MECH-2` test must shell out to something trivial (`true`, or a three-line script) —
  never to `serie` or any third-party TUI.
- **`petri` never writes the state file.** An `s`-to-rescan action (`ACT-2`) invokes
  `swab scan` as a child process; it does not grow its own writer. Invariant 1.
- **Agent-agnostic, always.** Every action and label is framed around "agent", never
  "Claude" — the schema already carries a copilot sensor, and a Claude-only tool is a
  much smaller thing than this wants to be.
- **Already deferred in `SPEC.md` §7**, so slot new thinking into these rather than
  re-proposing them: quota bars (`SURF-4`), inline card expansion (`MECH-1`),
  non-interactive `petri dash` (`SURF-5`), COLD as a one-line name list, responsive
  detail pane.

---

The full build history — how each `DONE` idea above actually got built, the design
mistakes found along the way, and the slice-by-slice "findings worth keeping" — lives in
`petri/IDEAS_LOG.md`.
