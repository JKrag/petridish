# Proposal — the **focus panel**: one renderer behind #30, #31, #32 and #33

**Status: proposal, not spec.** Nothing here has been decided or built. `petri/SPEC.md`
stays authoritative for what `petri` *is*; per `IDEAS.md`'s own rule, an idea becomes spec
only at the moment it moves into `SPEC.md`. This document exists to be argued with.

Covers: [#30](https://github.com/JKrag/petridish/issues/30) (`SURF-6`, dashboard detail
popup), [#31](https://github.com/JKrag/petridish/issues/31) (`SURF-7`, `petri --mini`),
[#33](https://github.com/JKrag/petridish/issues/33) (`SPACE-3`, a density tier above
roomy), informed by [#32](https://github.com/JKrag/petridish/issues/32) (`ACT-7`,
affordances not facts) and [#29](https://github.com/JKrag/petridish/issues/29) (`SURF-4`,
quota).

---

## 1. The claim

The overlap the issues share is not "a bigger view." It is **one question asked at three
different sizes**: *what is the state of this one project, and what can I do about it right
now?*

So build **one thing**: a `focus panel` — a pure function

```rust
fn focus_lines(radar: &Radar, proj_idx: usize, area: Rect, now: DateTime<Utc>)
    -> Vec<Line<'static>>
```

that renders a **content ladder** (§2) sized to whatever `Rect` it is handed, and mount it
in three places:

| Mount | Issue | What it is |
|---|---|---|
| Popup over the Dashboard | #30 | `MECH-1` overlay, `Space` to open, panel follows the cursor |
| The whole screen | #31 | `petri --mini`, one project, no list |
| The Browser's existing detail popup | #32 | the `Space`/#35 popup, re-pointed at this renderer |

That third mount is the load-bearing one and is proposed **deliberately, not smuggled in**:
it changes shipped behaviour. Re-pointing the Browser's popup at the focus panel delivers
`ACT-7` ("affordances, not facts") *in the Browser* — which is where the action keys
actually live today — as a consequence of building #30, rather than as a fourth project.

### #33 is the odd one out, and the proposal says so

A roomy card lives in an N-column grid and must stay uniform across every card in its
section. The focus panel owns a full-width rect and renders exactly one project. **They
share content, not layout.** The lush tier (§5) is "roomy plus the next two rungs of the
same ladder," reusing `zone_row`, `sparkline_glyphs` and the git/agent fact strings — it is
*not* "the popup, in a grid cell." Forcing one layout function across both would be the
kind of unification that costs more than it saves.

---

## 2. The content ladder

Ordered rungs. Every mount renders a **prefix** of this list, cut where the rows run out.
Ordering is the design decision; everything else is arithmetic.

| # | Rung | Rows | Content | Source |
|---|---|---|---|---|
| R0 | **identity** | 1 | glyph, name, dirty marker, `✎N`; right: `▲ waiting on you 4m` / `silent 12m` / `no agent` | shipped |
| R1 | **path** | 1 | `~`-abbreviated path | shipped |
| R2 | **git zone** | 1 | branch · commit age + 14d daily-commits sparkline | shipped (`zone_row`) |
| R3 | **agent zone** | 1 | agent · session + activity sparkline | shipped (`zone_row`) |
| R4 | **last event** | 1 | `last   Bash · 3 files · 14:18` | `agent.last_event` + `last_event_at` — **carried in the schema, never shown outside the feed** |
| R5 | **actions** | 2–3 | live affordances, dimmed when there is no target (§4) | `tools::registry()` + `ACT-9` |
| R6 | **recent** | 3–8 | per-project slice of the activity feed | `feed.rs`, filtered to one project |
| R7 | **repo** | 1 | `mine_last_commit_at` vs `last_commit_at` divergence, github url | schema, unshown |
| R8 | **tree** | 1–3 | worktree family via `parent_path` — siblings and their states | schema, unshown |

Quota is **not** a rung — it moved to the header on both screens (§6), because it is not a
fact about this project.

**R6 is the highest-value new content.** `feed.rs` already diffs successive `Radar`
snapshots; filtering that stream to one project is close to free, and it is the single
thing that makes #30 read as a *dashboard* rather than a taller fact sheet. Everything
above R5 is a fact sheet; R6 is a history.

**R5 before R6** deliberately: `FRAME-2` says petri is a router. When the panel is short,
"what can I do" beats "what happened."

---

## 3. Mockups

### 3.1 #30 — focus popup over the Dashboard (76 cols inside a 100-col terminal)

```
╭─ project-radar ───────────────────────────── ▲ waiting on you 4m ─╮
│ ~/repos/JKrag/project-radar                                       │
│                                                                   │
│ git    master ✎3 · commit 2h        ▁▂▁▅█▃▁▁▂▄▁▁▁▁         14d    │
│ agent  claude-code · sess 4b25919b  ▁▁▂▅█▇▅▂▁▁▃▆█▄▂▁       34m    │
│ last   Bash · 3 files · 14:18                                     │
│ repo   yours 2h · newest 2h · github.com/JKrag/petridish          │
│                                                                   │
│ RECENT                                                            │
│  14:22  ▲ waiting on you                                          │
│  14:18  claude-code Bash · 3 files                                │
│  14:11  claude-code Edit                                          │
│  13:57  user prompt                                               │
│                                                                   │
│ ACTIONS                                                           │
│  e edit nvim      g history serie      o remote github            │
│  f finder         y yank path          s rescan                   │
╰ space/esc close · j/k next project ───────────────────────────────╯
```

The popup is **not modal**, matching the Browser popup #35 already shipped: `j`/`k` keep
moving the Dashboard cursor underneath and the panel's content follows. That property is
already proven and is what makes this feel like focusing rather than like a dialog.

**But the Dashboard's cursor is not the Browser's cursor, and this needs deciding.** The
Browser's popup works trivially because its selection visits rows only (§3.1). The
Dashboard's sequence is `RUNNING` header → its rows → `IN FLIGHT` header → … and §3.2 calls
those header stops **load-bearing** — plus every collapsed-strip entry is its own stop. So a
cursor-following popup lands on a non-project stop on essentially every `j`/`k` walk, and a
`proj_idx: usize` has no way to say "the selection is a header."

Two honest options; **recommendation is (b)**:

- **(a) Skip header stops while the popup is open.** Reads well — `j`/`k` becomes
  "next project" — but it silently changes navigation semantics depending on whether an
  overlay is open, and it makes collapsed sections unreachable without closing the popup
  first. That is the same reachability trap §3.2 warns about.
- **(b) Render a rung-zero "nothing focused" state** on a header stop, naming the section
  and its count. Navigation semantics stay identical whether the popup is open or not, and
  the panel stays honest about what the cursor is on. This also inherits §3.1's existing
  requirement verbatim — the empty selection must be representable and must not panic —
  which the spec currently states only for the Browser's detail pane.

Either way the panel's signature has to admit the empty case (`Option<usize>`, or a
`FocusTarget` enum), not just a `usize`.

### 3.2 #31 — `petri --mini` at 60×20 (the corner-pane case SURF-7 describes)

```
 petri · project-radar      5h 16% · 7d 1% · ▲ 4m
══════════════════════════════════════════════════
 ● project-radar ✎3                 waiting on you
   ~/repos/JKrag/project-radar

 git    master ✎3 · commit 2h    ▁▂▁▅█▃▁▁▂▄▁  14d
 agent  claude-code · 4b25919b   ▁▂▅█▇▅▂▁▃▆█▄  34m
 last   Bash · 3 files · 14:18

 RECENT
  14:22  ▲ waiting on you
  14:18  claude-code Bash · 3 files
  14:11  claude-code Edit
  13:57  user prompt

──────────────────────────────────────────────────
 e edit · g history · o remote · q quit
```

### 3.3 `--mini` at 36×10 (a real narrow split)

```
 petri · project-radar        ▲ 4m
──────────────────────────────────
 ● project-radar ✎3
   waiting on you 4m

 git    master · 2h   ▁▂▁▅█▃▁ 14d
 agent  claude-code   ▁▅█▇▅▂▁ 12m
 last   Bash · 3 files

 e g o · q
```

R1 (path), R6 (recent), R7/R8 and R9 are gone. The actions rung degrades to bare keys — the
labels are the first thing to go, since the footer's job here is to say *the keys still
work*, not to teach them.

### 3.4 `--mini` at 24×6 (the floor)

```
● project-radar ✎3
▲ waiting on you 4m
master · commit 2h
▁▂▅█▇▅▂▁▃▆█▄▂▁▁▁▁▁▁▁
q quit
```

No header, no rule, no zone labels — at 24 columns a 7-cell label costs 29% of the line.
The sparkline stays because it is the highest information-per-cell element on the screen.

### 3.5 Below the floor

```
petri --mini needs 24x6
```

Naming the actual required dimensions, per `SPEC.md` §4.5's "clip, or show a message" and
the same honesty rule as §4.4's missing-state-file message.

---

## 4. #32 — what "affordances, not facts" actually means here

`ACT-9`'s second axis is still open: a project can lack a *target* (no `github_url`, no
session) even when the tooling is installed, and that currently surfaces as a transient
notice after you press the key. In the focus panel it becomes visible **before** you press
it:

```
 ACTIONS
  e edit nvim      g history serie      o remote ─ no url
  f finder         y yank path          s rescan
```

Three states per action, and none of them is color-only:

| State | Render | Why |
|---|---|---|
| live | `e edit nvim` — key accented, tool name dim | the tool resolved and the target exists |
| no target | `o remote ─ no url` — whole entry `DIMMER`, `─` + words | `Resolution::NoTarget` |
| no tool | entry omitted entirely | `Resolution::NoTool`; the footer must not advertise a key that does nothing (`SPEC.md` §5) |

Naming the *resolved tool* (`serie`, `nvim`) is the other half of ACT-7: it turns the
registry from invisible machinery into a visible affordance, and it makes the shifted
re-pick key (`ACT-11`) discoverable for the first time.

---

## 5. #33 — the lush tier, and the conflict it has with the feed

Lush = the roomy card plus rungs **R4 (last event)** and **R7 (repo)**, and wider
sparklines (`agent_sparkline_width_for` already scales; the git sparkline stays pinned at
14 days because there is no more daily history to show). Six content rows instead of four.

**The conflict, stated rather than discovered later.** `SPACE-1`'s feed grows to fill the
slack, but it is not unbounded: §3.2 bounds it at `events + 2` (the rule and the label), and
it already yields entirely to project rows whenever a section was skipped or truncated. So
on a quiet fleet both features fit and nothing competes. On a **busy** fleet — many events,
tall terminal, RUNNING expanded — they want the same rows, and **the ordering between them
is currently unstated**. Whichever code happens to claim first wins, which is not a design.

**Recommendation:** give RUNNING cards a **bounded** first claim on surplus (+2 rows per
card, i.e. exactly the lush tier), then hand the rest to the feed under its existing
`events + 2` rule. A bounded claim ordered ahead of an elastic one is what lets both
features coexist on a busy fleet; the reverse ordering starves #33 exactly when there is
most to show. This needs to be an explicit decision in `SPEC.md` §3.2, not an emergent
property of whichever code runs first.

---

## 6. #29 — quota goes in the header (revised: it is not a rung)

**Correction to a shared assumption: the quota *sensor* is already built, shipped and
tested.** `swab/src/sensors/quota.rs` reads Claude Code's own
`~/.claude/last-status.json`, parses `rate_limits.five_hour` / `.seven_day` /
`context_window`, degrades field-by-field, and populates `Radar.quota` on every scan
(`swab/src/scan.rs:459`). It was ported from `sensors/quota.py` along with everything else —
it is the *display* half that was deferred. **#29's MVP is therefore display-only: no sensor
work, no new dependency, no schema change.** That materially shrinks it, and it is the
strongest reason to take your "let's make room for it" now rather than later.

**Accepting the header placement.** It is not a fact about this project, so it does not
belong in a per-project rung. `ZONE_LABEL_WIDTH`-labelling it `fleet` inside the panel (my
previous suggestion) technically told the truth but still spent a full body row on a global
number. The header already *is* the global-context row on both screens — project count,
clock, scan duration — so quota joins facts of its own kind, and costs zero body rows:

```
 petri · dashboard        14 projects · 5h 16% · 7d 1% · 14:22 · scan 0.3s
═══════════════════════════════════════════════════════════════════════════
```

At your measured 13 characters for `5h:16% 7d:1%` this is cheap enough to be
unconditional. Rendering `5h 16% · 7d 1%` (spaces, `·`) rather than `5h:16% 7d:1%` matches
the separator vocabulary the header already uses; that costs one extra cell and I'd take it,
but it is a pure taste call.

**Note a deliberate divergence:** `dashboard.rs`'s `DashPlan` doc comment already
anticipates this feature as *"a quota-gauge rail"* carved out of the `fleet` rect. I'm
recommending against the rail — a rail permanently spends a column of width on ~13
characters of data, on the one screen whose whole problem (`SPACE-*`) is that it runs out of
room. If that comment stays, it should be amended rather than left contradicting the built
thing.

**Degradation ladder** for the header's right side, most-droppable first, since it cannot
wrap (same constraint §3.1's filter chip already lives under):

`scan 0.3s` → `N projects` → the clock → `5h 16% · 7d 1%` compresses to `16%/1%` → quota
drops entirely. Quota outranks the clock deliberately: a clock is available everywhere, and
the burn number is the thing you opened this pane to keep half an eye on.

Absent quota (`Radar.quota: None`, which `hostile.json` covers) omits the segment entirely —
never `0%`, which reads as "you have used nothing" rather than "we do not know."

### The `ctx:` field needs a decision, and it is the interesting one

You are right that context is session-specific, and the schema agrees with you in the worst
possible way: `context_used_pct` is parsed from `context_window.used_percentage` in
`last-status.json` — a **single file that whichever Claude Code session wrote last owns**.
So the value is neither fleet-wide nor attributable to the project you are looking at. It is
"the context of the most recent session on this machine."

Rendering that as `ctx 13%` in a `--mini` pane pinned to `project-radar` would be a lie
roughly whenever you have two agents running — which is the exact situation petridish exists
for. Three options, in order of how much I like them:

1. **Omit `ctx` from the header for now.** Ship `5h`/`7d`, which are genuinely
   account-global and therefore honest anywhere. Cheapest, and correct.
2. **Show it only in `--mini`, only when the panel's project matches the session that wrote
   `last-status.json`** — but the file carries no session id today, so this needs a sensor
   change to record one, and it is then only sometimes present.
3. **Make context per-project properly** — a real sensor reading each transcript's own usage
   records, landing on `AgentState` rather than `QuotaState`. That is the version worth
   having (it belongs next to `sess 4b25919b` in the agent zone, not in a global header),
   and it is a `swab` project of its own, not part of #29's MVP.

Recommendation: **(1) now, (3) as its own issue.** Filed as `DATA-5` in `IDEAS.md`.

---

## 7. Keys — `Space` it is (revised: my objection was wrong)

**Withdrawn.** I argued `Space` was unavailable because `SPEC.md` §3.2 calls it
load-bearing for reopening collapsed sections. Re-reading the actual rule, that protection
comes from **header stops being selectable**, not from `Space` behaving identically on every
kind of stop. Headers are selection stops precisely so a collapsed section is reachable;
`Space` on a *header* is what reopens it, and that is untouched by anything below. My
objection conflated the two.

**Adopted: `Space` is contextual quick-look, on both screens.**

| Selection | `Space` today | `Space` proposed |
|---|---|---|
| Dashboard, section header | toggle that section | **unchanged** |
| Dashboard, collapsed-strip entry | toggle that section | **unchanged** |
| Dashboard, project row | toggle the *containing* section, then move the cursor to its header | **open/close the focus popup** |
| Browser, any row | toggle the detail popup | **unchanged** (now the same renderer) |
| Any, popup open | — | close it |

This is better than `d` on four counts, and one of them I had not seen:

1. It is the Mac quick-look gesture, which is the right mental model for a
   non-modal panel that follows the cursor.
2. **It makes the two screens agree.** `Space` already opens a detail popup in the Browser.
   Under my `d` proposal the shipped Browser binding had to be retired to avoid two keys for
   one action; under yours there is nothing to retire — the Dashboard grows *toward* the
   Browser's existing behaviour instead of both moving to a third key.
3. It costs no new key on a screen whose footer is already tight.
4. `Esc` closes, symmetric with the Browser, unchanged.

**The one real change is removing a special case, not adding one.** Today `Space` on a
project row reaches *past* the row to toggle its section and then relocates the cursor — the
only binding in either screen that acts on something other than what the cursor is on, and
the only one that moves the cursor as a side effect. That is precisely the "special case"
you named. Under this proposal every stop acts on itself: header toggles the header's
section, row opens the row's project.

**What it costs, stated plainly:** collapsing a section while the cursor is on one of its
rows becomes `k`-to-the-header-then-`Space` instead of one keypress. Cheap, and it is the
rarer of the two intentions by a wide margin — you collapse a section once per session and
inspect projects continuously.

`Enter` is unaffected: on a header it still toggles, on a row it still jumps to the Browser.
`d` and `D` stay unbound.

---

## 8. `--mini`'s hardest question: which project?

`SURF-7`'s own text answers it — *"a live view of the run currently on your screen, i.e. in
this terminal window/tab."* So:

1. **Default: no argument — `petri --mini` is the project you are standing in.** Resolve
   `$PWD` through the same `resolve_root()` logic the scanner uses, so a monorepo subdir
   collapses to its project (scanner invariant 3), then look that root up in
   `projects.json`. Being friendly about it costs nothing: walk up to the git toplevel
   first, so running it from `src/sensors/` still finds the project. Using the scanner's own
   resolver rather than a second path heuristic is the point — two different answers to
   "which project is this directory" is exactly the bug class invariant 3 exists to prevent.
2. **Override:** `petri --mini <PATH|NAME>` for a pinned pane.
2b. **Re-resolve every tick, by path/name — never hold an index across a reload.** §4.3's
   own rule, learned from a live bug: the scanner re-sorts `radar.projects` on every scan,
   so a cached `usize` silently starts pointing at a different project. A `--mini` pane
   pinned in a corner for days across thousands of rescans is the worst possible place for
   that failure, because there is no list on screen to make the swap obvious.
3. **cwd is not a known project:** an honest state, not a crash and not a silent fallback
   to some other project — following §4.4's precedent of naming the problem and what to do:

   ```
   petri --mini: ~/scratch/foo is not in projects.json
                 run 'swab scan', or: petri --mini <name>
   ```

**Product shape:** alt screen, same event loop, same mtime poll. It is a session you leave
running, not a summon-and-exit tool, so the alt screen is right — but it should exit
cleanly enough that a `--once` variant (`SURF-5`) is a trivial follow-on, since one
off-screen buffer render serves both.

---

## 9. Clutter audit (run on *this* proposal, not on the existing screens)

- **Border-nesting depth: 1 for the popup, 0 for `--mini`.** The popup gets exactly one
  border because it must separate itself from the Dashboard behind it. `--mini` gets none —
  the terminal edge already frames it, and at 36 columns a border costs 6% of every line
  for zero information. Inside the popup there are **no sub-borders**: `RECENT` and
  `ACTIONS` are dim uppercase labels plus a blank row, the same idiom `feed.rs`'s
  ` ACTIVITY` already uses.
- **Signals per piece of state:** waiting-on-you is `▲` + `DANGER` + the words "waiting on
  you 4m" — glyph, color, text. That is three, and it is the deliberate maximum, because it
  is the one state the whole product exists to surface and it must survive `NO_COLOR`. It
  does **not** additionally get a border color change or a row marker. Every other state
  gets at most two signals.
- **Always-on markers:** none. The action rows use the key itself as the leading token —
  which is information, not texture. No `▶`, no bullets.
- **Chrome-to-data ratio:** at 76×18 the popup spends 2 rows on border and 2 on section
  labels ≈ 22%. At 36×10 that must fall: the header rule goes below 12 rows, the zone
  labels go below 30 columns, and at 24×6 chrome is one footer line ≈ 17%.
- **Removal test:** the `path` rung (R1) is the first thing cut, above the fold, because
  `--mini` is run *from* the project — the path is the one fact the user already knows.

---

## 10. Floor pressure test

| Size | Focus popup (#30) | `--mini` (#31) |
|---|---|---|
| 120×40 | full ladder R0–R8, `RECENT` at 8 rows | full ladder + R9 quota |
| 100×30 | R0–R7, `RECENT` at 4 rows | R0–R6 + R9 |
| 80×24 | R0–R6 (repo/tree drop), `RECENT` at 3 | R0–R6, `RECENT` at 3 |
| 60×20 | R0–R5, no `RECENT` | §3.2 mockup |
| 48×14 | popup too small → **falls back to full-screen focus** | R0–R5, actions as bare keys |
| 36×10 | full-screen focus | §3.3 mockup |
| 24×6 | full-screen focus | §3.4 mockup |
| <24×6 | `terminal too small` | `petri --mini needs 24x6` |

The 48×14 row is the payoff of building one renderer: **when the popup no longer fits, `Space`
renders the same panel full-screen instead of refusing.** No new code path — it is the
`--mini` mount with a different lifetime. The Dashboard's current answer at that size would
be to have no detail surface at all.

A note on the Dashboard's popup floor specifically: the popup must never exceed ~80% of the
terminal in either axis, or "overlay on the Dashboard" stops meaning anything and the user
would be better served by the full-screen fallback. That ratio, not an absolute size, is
what should drive the switch.

---

## 11. "What more can we show if we have room?"

### Already in `projects.json`, shown nowhere or only in aggregate

Free to add — no crate boundary crossed, no sensor work:

1. **`agent.last_event` + `last_event_at`** — what the agent last *did*. Currently only
   reaches the screen via the feed, i.e. only for projects that changed between two scans.
2. **A per-project activity history** (R6). `feed.rs` already computes it fleet-wide.
3. **The full `agent_activity` ring** — 60 samples ≈ one hour; roomy cards show ~20. A
   focus panel at 100 columns can show the whole ring, which is the first time "how has
   this run behaved over the last hour" is actually visible.
4. **The full 14-day `daily_commits`**, un-elided.
5. **`mine_last_commit_at` vs `last_commit_at`** — the "someone else pushed here" signal.
   Cheap, and genuinely unavailable anywhere else in the UI.
6. **`waiting_since` as a duration** — "waiting on you **4m**" instead of a bare `▲`. Four
   minutes and forty minutes are very different situations and the flag conflates them.
   Note that `petri` must keep re-deriving `waiting_latch_live` at render time (§3.2's
   existing rule) — a duration makes a dead latch *more* conspicuous, not less.
7. **The worktree family via `parent_path`** — siblings and their buckets, which today you
   can only reconstruct by scanning the Browser list by eye.
8. **`category`** — untouched by any surface today, and issue #1 wants to categorize by
   search root.
9. **Age of the data itself** — `Radar.updated_at` relative to now. The Dashboard has a 24h
   staleness banner; a `--mini` pane pinned in a corner for days is exactly where a
   *quietly* stale reading does the most damage, so it should show scan age continuously,
   not only past a threshold.

### What this proposal actually builds, and what gets deferred

Per your "prioritize what you think is most important; document the rest in `IDEAS.md`":

**In the focus panel now** — 1, 2, 5, 6 (they are rungs R4, R6, R7 and R0 respectively).
Rationale: 2 is what makes it a dashboard instead of a fact sheet; 6 is the product's whole
reason for existing and is currently under-rendered; 1 and 5 each buy a genuinely
unavailable fact for one line.

**In `--mini` only** — 9. A pinned pane is the one place a silently-stale reading does real
damage; the Dashboard's 24h banner already covers the other case.

**Deferred to `IDEAS.md`** — 3 (full ring; the width-scaling sparkline already gets most of
it, and the extra samples are the least legible part of the ring), 4 (full 14-day commits;
the card already shows all 14 days), 7 (worktree family, R8 — real value but it needs a
rollup design of its own, and #1's categorisation work will touch the same code), 8
(`category`, which belongs with #1 rather than here).

### Requires a `swab` sensor change (boundary flagged, not proposed here)

Filed in `IDEAS.md` under a new `DATA-*` prefix — these are *facts nobody collects*, which
is a different kind of idea from the existing `MECH`/`ACT`/`SPACE`/`SURF` axes and was
previously homeless:

- **`DATA-1` ahead/behind vs upstream** — you called this the valuable one and I agree. A
  `gix` revwalk against the tracking branch, no network, and it lands directly in the `repo`
  rung (R7), which is otherwise a thin row. This is the one I'd build next after the panel.
- **`DATA-2` PR / CI state** — agreed on both counts: nice to have, and it stretches the
  seam hardest. It is the first thing in the project that would make `swab scan`'s latency
  depend on a network round-trip, on a 60s timer, per project. If it is ever built it
  probably wants its own cadence and its own cache, not a slot in the scan tick.
- **`DATA-3` stash count / unpushed-commit count** — cheap once `DATA-1`'s revwalk exists.
- **`DATA-4` per-session token usage.**
- **`DATA-5` per-project context window** — §6's option (3); the honest version of `ctx%`.

---

## 12. Build order, if this is accepted

Each step independently shippable and verifiable, per the repo's one-commit-per-verified-
slice discipline:

1. **`focus.rs`** — the ladder as a pure function over `(&Radar, idx, Rect, now)`. Pure-state
   unit tests + `TestBackend` snapshots at the §10 sizes against all four fixtures,
   `hostile.json` included. No mount yet.
2. **Mount #30** — `MECH-1` popup + contextual `Space`, non-modal, cursor-following, with the
   below-threshold full-screen fallback. Adds `focus.rs` to `glyph_portability.rs`'s module
   list (§4.2's "adding a render module means adding it here" — the rule that was missed for
   `feed.rs` and `picker.rs`).
3. **Re-point the Browser popup** at `focus.rs`. Its `Space` binding is unchanged; only
   what the popup renders changes. Closes #32.
4. **`petri --mini`** — cwd resolution, the not-a-project state, alt-screen lifetime.
   Closes #31.
5. **Lush tier** — R4+R7 as extra roomy-card rows, plus the §5 surplus-priority decision
   written into `SPEC.md` §3.2. Closes #33.
6. **Quota in the header** — both screens, `5h`/`7d` only, with §6's elision ladder. Closes
   the MVP half of #29; the "hand off to a dedicated token TUI" half stays open as a
   registry entry, and `ctx%` stays open as `DATA-5`.

Steps 1–3 are one coherent piece of work; 4–6 are independent afterwards.

**Step 6 can jump the queue.** Now that the sensor turns out to be built (§6), the header
quota is a self-contained change to `header_lines` plus an elision ladder — no dependency on
`focus.rs` at all. If you want it soon, take it first; it is the smallest thing on this list.

---

## 13. Glyphs — cheap; Nerd Fonts are a different question (revised)

**You are right that I overstated the cost, and I should correct the framing.** An
allowlist entry is one line in `petri/tests/glyph_portability.rs`'s `ALLOWED` table plus a
sentence of reasoning; the gate then proves `UnicodeWidthChar::width() == Some(1)`
automatically. That is not expensive, and §4.2's own text is explicit that the criterion is
computed cell width — *not* a vintage rule, and not a reason to be stingy. **Adding plain
Unicode glyphs that widen the vocabulary is fine and we should feel free to do it.** The
budget below is a checklist, not a deterrent.

This proposal's actual cost, checked against the current table:

- **Already cleared:** `·` `×` `—` `…` `→` `─` `│` `═` `▁`–`█` `▲` `▌` `▼` `○` `●` `✎`.
  Note `U+2588`'s existing reason string already reads *"sparkline level 8/8, **quota bar
  filled**"* — someone anticipated §6 and pre-cleared it.
- **New, only if the popup gets rounded corners:** `╭ ╮ ╰ ╯` — four entries, trivially
  single-width. But check first: `picker.rs` and `help.rs` both draw popups with ratatui's
  default `Borders::ALL`, whose glyph set is `┌ ┐ └ ┘ ─ │` — and `┌ ┐ └ ┘` are **not** in
  `ALLOWED` today. The gate only scans string literals in the listed modules, so
  ratatui-generated border characters slip past it. That is a real (small) hole in the gate,
  found while writing this, and worth a line in `IDEAS.md` regardless of what this proposal
  does.
- **Zero-cost by design:** §3's quota bars use `█` plus spaces, so `▏▎▍▌▋▊▉` (U+258F–2589)
  stay unneeded. If someone wants sub-cell precision later, adding seven entries is a
  half-hour, not a blocker — I just don't think 8× precision on a 12-cell bar is worth
  anything.

### Nerd Fonts: the real constraint, and it is not the allowlist

Your instinct is right that a branch/GitHub/agent glyph would buy real estate on a 36-column
pane — `` for a branch is 1 cell where `master` is 6. The blocker isn't width, it's that
**no terminal emulator exposes "a Nerd Font is installed and active" as a queryable
signal.** Not kitty, WezTerm, iTerm2, Alacritty, Ghostty or Windows Terminal — fonts are
client-side rendering and the terminal protocol has no capability query for them. Font
fallback also means a *declared* font can silently substitute per glyph. So there is no
detection to write; the whole ecosystem is opt-in:

- **eza** — `--icons=always|automatic|never`, where `automatic` gates on stdout being a TTY,
  not on font presence. It simply trusts the user.
- **lazygit** — `gui.nerdFontsVersion: '2' | '3' | ''`, defaulting to empty (no icons).
  Version-pinned because Nerd Fonts v3 *moved* the Material Design Icons codepoints
  (`F500–FD46` → `F0001+`) to stop colliding with CJK — a real breaking change.
- **starship** — ships all three rungs as named presets: `nerd-font` / `no-nerd-font` /
  `plain-text-symbols`.

**Recommendation, as its own idea rather than part of this proposal:** an `icons` key in
`petri.toml` (`"none"` default / `"unicode"` / `"nerdfont-v3"`), with a `glyphs` indirection
module so every render site asks for a *semantic* glyph (`glyph::branch()`,
`glyph::agent(kind)`) rather than embedding a codepoint. That is the same indirection the
palette already has in `theme.rs`, it keeps the portability gate meaningful (each tier is
its own allowlist), and it is what makes a per-agent glyph for claude-code vs copilot
possible at all. Filed as `SPACE-6` in `IDEAS.md`.

Note the interaction with the gate: a Nerd Font codepoint is in the Private Use Area, where
`unicode-width` reports `Some(1)` — so it would *pass* the width check while being
completely invisible on a machine without the font. That is precisely the silent-blank
failure mode §4.2 exists to prevent, which is the strongest argument for the opt-in tier
being explicit rather than detected: the user asserting "I have the font" is the only signal
that actually exists.
