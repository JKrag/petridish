# `petri` — idea backlog: build log

This is the companion to `petri/IDEAS.md`. That file is a pure backlog: what's still
open, and a one-line pointer for what's already built. This file is where the pointer
leads — the slice-by-slice narrative of *how* each built idea got built, the design
mistakes found along the way, and the numbered "findings worth keeping" that would
otherwise be lost once the corresponding IDEAS.md entry got compressed to one line.

Split out 2026-09-06 because IDEAS.md had grown to ~1000 lines and the implementation
history was burying the still-open ideas. Nothing below was rewritten, only moved and
regrouped by slice; see git history on `petri/IDEAS.md` for the original.

---

## Slice 1 — `MECH-1`/`MECH-2`/`MECH-3`, `ACT-1`/`ACT-3`/`ACT-4`/`ACT-8`/`ACT-9`

Landed 2026-09-01/02 (`e99cc8d`..`bd87b9b`).

**`MECH-1` — Popup / overlay frames.** Feasibility: trivial, not a terminal-capability
question at all. Render `Clear` into a centered `Rect`, then the content block on top,
last in the draw call — ratatui painting its own cells, nothing negotiated with the
terminal emulator. Anything renderable as a screen is renderable as a popup. Customers:
help screen (`?`), confirmation dialogs, quota detail, per-project detail in place on the
Dashboard.

**`MECH-2` — Suspend-and-exec (hand the terminal to a child process).** Feasibility: easy,
~30 lines, and it is the *right* answer rather than a compromise — a git-graph browser
wants the whole screen, not a 40-column pane.

```rust
// leave the TUI
disable_raw_mode()?;                       // raw mode first: more side effects
execute!(stdout, LeaveAlternateScreen)?;
// hand over stdin/stdout/stderr wholesale
let status = Command::new(prog).args(args).current_dir(&path).status();
// take it back
enable_raw_mode()?;
execute!(stdout, EnterAlternateScreen)?;
terminal.clear()?;                         // mandatory — see below
```

Three failure modes, written into `petri/src/exec.rs` as code comments rather than left
here:

1. **`.status()`, never `.output()`.** `output()` captures stdio, so the child renders
   into a pipe: you see nothing and it looks like a hang.
2. **Reinit on the way back is mandatory**, but `terminal.clear()` is the wrong way to do
   it on ratatui 0.30 — it snapshots the cursor via a blocking DSR query at the worst
   possible moment. `Terminal::resize` does the same buffer invalidation with no
   round-trip. The universally-given advice ("clear on return") is right about the *what*
   and wrong about the *how*.
3. **Restore on the child's failure path too** (binary missing, child panics), or a
   missing tool leaves the user in raw mode with no shell echo.

`petri` does not enable mouse capture, so there is no mouse mode to tear down and
restore; if mouse support is ever added, it joins both halves.

**`MECH-3` — Spawn-and-detach (for GUI targets).** Distinct from `MECH-2` and needed
alongside it. `code $folder`, `open -a`, a browser URL: these must **not** take the
terminal and must **not** be waited on. Different call shape (spawn, null stdio, don't
wait, don't tear down the TUI), so the external-tool registry (`ACT-1`) carries a
per-entry mode: `Terminal` (suspend, wait) vs `Background`.

**`ACT-1` — An external-tool registry, not hardcoded keys.** Each action is data: name,
key, probe (how to tell it is available), argv template, exec mode (`MECH-3`), and an
ordered fallback chain. Three payoffs: the footer can honestly advertise what a key will
actually do, the fallback chain becomes declarative, and the whole table is overridable
in `petri.toml` — the concrete form of "petri is a router, not a reimplementation of
every tool it points at" in the code.

**`ACT-3` — Git history with a graceful fallback chain.** `serie` → `lazygit` → `gitui` →
`tig` → plain `git log --graph --oneline --decorate --all`. The last one is not a
degraded consolation prize: git pages through `less` by itself when stdout is a tty, so
the *interaction* is the same as the fancy tools — full screen, `q` returns to petri.
That means `g` can always be bound and always advertised; tool detection only decides
*which* graph you get, not whether the key exists. One measured caveat: git's default
pager is `less -F -X`, and `-F` makes less print-and-exit when the output fits one
screen, so a short history would flash past instead of behaving like a TUI. The fallback
pins its own pager for that one invocation (`git -c core.pager='less -R' log --graph …`)
rather than inheriting the user's config, so the interaction is uniform across the whole
chain.

**`ACT-4` — "Open in editor" and the folder-vs-file problem.** The real question is not
detection but what we hand the editor: a directory. Multi-file/GUI editors (`code`,
`cursor`, `zed`, `subl`, `idea`, `windsurf`) take a folder natively — `MECH-3`. Most
terminal editors handle a directory fine too (`vim`/`nvim` open netrw, `hx` opens its
file picker, `emacs` opens dired) — `MECH-2`. The genuinely awkward case is small: `nano`
and friends, which have no directory concept. Measured on this machine: `$EDITOR` and
`$VISUAL` are both unset while `code` is on `PATH`, so the env-var chain frequently
yields nothing at all, and a probe-based default is doing more work in practice than the
`nano` edge case. Resolution order: an explicit `editor` entry in `petri.toml` →
`$VISUAL` → `$EDITOR` → a best-available probe. The config override is what turns the
awkward case into a non-problem.

**`ACT-8` — First-run tool picker popup.** When an action has several plausible targets
on this machine, don't guess and don't make the user find the config file first. On the
first invocation of that action, open a `MECH-1` popup listing the probed candidates,
plus an `Other — specify path…` entry, with a footnote saying where the choice is
stored. Persist to `petri.toml` and never ask again. This turns `ACT-1`'s registry from a
static table into a resolver with state:

- **Only ask when the choice is genuinely ambiguous.** Zero candidates → go straight to
  `Other`. Exactly one → use it silently.
- **Order candidates opinionatedly**, best-guess first.
- **Offer a re-pick key** rather than relying on the footnote — see `ACT-11`.
- **Re-ask when the stored choice disappears** from `PATH`.
- **Tests must never see the popup** (pre-seed `petri.toml` in the fixture) — `ACT-11`'s
  test *inverts* this, because there the popup is the feature.

**`ACT-9` — Action availability has two independent axes.** Worth separating because
they fail differently: tool availability (per machine — resolved by probing and fallback
chains) decides whether a key is bound at all; target availability (per project — this
project has no `github_url`, so `o` has nothing to act on) decides whether it's live for
the current row.

Three findings from this slice, kept because they will outlive the code:

1. **`Candidate::fallback` was not in the original design and the design was broken
   without it.** `git` is on every machine, so a fallback that counted toward ambiguity
   would have made the picker fire for every user, forever — invisible on a machine with
   four git TUIs installed, which is where it was designed.
2. **`Terminal::clear()` is the wrong way to come back from `MECH-2` on ratatui 0.30** —
   see the `MECH-2` writeup above. The universally-given advice is right about the *what*
   and wrong about the *how*.
3. **Adding a field to `Prefs` silently wiped the `[tools]` table on every screen
   switch**, because three `Prefs { .. }` literals in `lib.rs` each named the new field as
   empty. No unit test could have caught it; `prefs::save` was called correctly with
   exactly the struct it was handed. Fixed at the root by keeping the loaded prefs alive
   and mutating in place.

Still open from this slice: the `ACT-4` `Background` path has never been exercised
against a real GUI editor on a desktop — only unit-tested.

---

## Slice 2 — `ACT-11`, the re-pick key

Landed 2026-09-02 (`b65c6ec`, `1a3bdd1`). `G`/`O`/`E` open the picker in re-pick mode:
`Enter` runs the highlighted tool once, `D` makes it the default and runs it, `Esc` does
neither. Shifted keys are derived from `action.key`, so a new registry entry gets one for
free.

A refinement of `ACT-8`'s re-pick bullet, from the daily-use case that motivates it: *"I
use `serie` as my everyday git viewer, but just this once I want `lazygit`."* That is not
a request to change a default — it is a request to bypass one. If the shifted key were
purely "reconfigure this action", using it would silently cost you your real default
every time you wanted a one-off, and you would then have to re-pick back. So the popup
carries two verbs:

| key | meaning |
|-----|---------|
| `Enter` | launch the highlighted tool **once**; stored default untouched |
| `D` | make the highlighted tool the **new default**, and launch it |
| `Esc` | cancel; launch nothing, change nothing |

`Other — specify path…` needed care, because inside the text field `D` is a literal
character and cannot also be a verb. The field inherits the verb you opened it with:
`Enter` on the `Other` row opens it in run-once flavour, `D` opens it in set-default
flavour, and `Enter` in the field commits with whichever it is. The footer names the live
one.

The first-run popup (`ACT-8` proper) keeps its single verb — there is no default yet to
preserve.

Three consequences worth pinning:

- **`Shift`+`Enter` is not available.** Terminals do not report it distinguishably from
  plain `Enter` without the kitty keyboard protocol, and `petri` pushes no
  `KeyboardEnhancementFlags`. `D` is a plain, portable key the footer can honestly
  advertise.
- **The shifted key must not route through `Resolution::Ambiguous`.** That variant only
  appears when the choice is *unresolved*; re-pick needs its own entry point that lists
  every installed candidate regardless of ambiguity.
- **Bind the shifted variant for every registry action, not just `g`.** A per-action
  exception list is the thing to avoid.

Two findings worth keeping:

1. **`Resolution::Ambiguous` was the wrong door, and it fails silently.** Re-pick's
   premise is that the choice IS resolved; `resolve` collapses that case to `Ready` and
   throws the candidate list away. Routing `G` through it would have produced an empty
   popup on precisely the machines the feature is for. The PTY test seeds a resolving
   default so a regression back to `resolve` cannot pass.
2. **A parameter removed is worth more than a test added.** `run_action` used to re-read
   `prefs.tools`, correct only because every caller wrote prefs first. Rather than test
   that a one-off doesn't, `prefs` was removed from its signature entirely — now it
   *cannot* consult a stored answer, which no later refactor can quietly undo.

The build ran as an AFK delegation to the local model and is worth recording honestly.
The model produced `tools.rs` and its eight tests, and most assertions were real ones —
registry order and the `fallback` flag, not just counts. But the round failed its gate,
and the audit that followed found the thing the gate could not: `launch_for`'s
known-candidate test used `serie`, which takes no arguments, so it proved args *survive*
without ever proving `{path}` is still *substituted*. The test name promised the
coverage; the body didn't have it. Transferable lesson: check assertion bodies against
the spec, because a test that passes and is named correctly can still be
under-asserting. The model also froze twice with an undiagnosed hang, and the
picker/wiring half was written directly after the job escalated.

---

## Slice 3 — `ACT-10`, the invisible filter

Landed 2026-09-02. The header now carries a filter chip whenever a query is live:
`/query` plus `<matched> of <total>`, bright with a block cursor while typing, dim
without one once `Enter` has closed the input.

Found while gating the action keys: `BrowserState.filter_query` is stored and applied,
but `browser::render` never displays it — the only visible evidence that a filter is
active is that the list got shorter.

Five things worth keeping:

1. **The chip belongs in the header, not the footer.** Replacing the footer with a
   filter prompt trades a permanently-useful surface for a transient one: the keymap is
   worth *more* while you are in an unfamiliar mode, not less.
2. **The interesting state is the *closed* one.** While you type, the query is at least
   implied by the keys you just pressed. The state this was actually about is after
   `Enter` — a naive fix (draw the input line while the input is open) leaves that state
   exactly as broken as before.
3. **The header cannot wrap, so the chip needed a width budget.** At 40 columns an
   over-long query pushed `0 of 15` off the right edge — losing exactly the disambiguator
   the whole idea is about. The count is laid out first and the query is elided into the
   remainder.
4. **The mode flag had to move to be renderable.** `in_filter_input` was a local in
   `poll_loop`; `render` takes `&BrowserState` and nothing else. Mirroring it would have
   been two sources of truth for one mode — the same shape as slice 1's finding 3.
5. **The display fix exposed a keymap gap: `Backspace` was unbound.** A mistyped query
   could only be abandoned and retyped, invisibly. Bound in the same slice, with a PTY
   test, because the real terminal byte is `0x7f` (DEL) rather than `0x08` and nothing
   below layer 3 proves that decodes.

This slice also retired `SPEC.md`'s "the footer may only advertise keys that are actually
bound" as a hard requirement — it was machine-generated, never a decision anyone made,
and had started to shape work (the header-vs-footer choice above was originally argued
from the rule rather than from the design).

---

## Slice 4 — `SPACE-1`, the activity feed

Landed 2026-09-03. Surplus height below the fleet now carries a rolling record of what
the fleet has been doing, newest first, agent-agnostic. `SPEC.md` §3.2 has the behaviour;
this section has the reasoning.

Ten things worth keeping:

1. **`events.ndjson` cannot back this feature, and that is not a detail.** The original
   idea assumed it is a log. It is a *hand-off buffer*: `swab::events::read_and_compact`
   truncates it on every scan tick, so at any moment it holds at most one tick's events
   and a reader would race the scanner's truncation. The same facts live durably in
   `projects.json`, so the feed diffs successive `Radar` snapshots instead — race-free,
   no second file, and `petri` stays a pure reader. Cost, accepted openly: the feed
   advances at scan cadence, never "live", and only the newest event per project per tick
   is visible.
2. **Surplus height and truncation genuinely co-occur — the yield rule is load-bearing.**
   The obvious implementation ("spend whatever `plan_layout` left over") is wrong: a
   roomy RUNNING card spans 7 rows, so a 20-row budget fits two, reports
   `truncated_remaining`, and strands five rows. Drawing a feed in those five while
   projects sit behind a `… +N more` marker inverts the priority order. `feed_rows_for`
   refuses whenever anything was skipped or truncated.
3. **The reload-order trap was designed out, not tested around.** `lib.rs`'s reload
   branch destroys the previous snapshot; `ingest` needs it, so assigning first yields a
   feed that never grows a row. `absorb_snapshot` takes ownership of *both* snapshots, so
   the bad order is unrepresentable.
4. **Testing against the real `projects.json` found two things no fixture did.**
   Rendering against 79 real projects showed (a) a bare clock cannot express "yesterday",
   fixed with `MM-DD` for earlier-day rows; and (b) `agent.last_event` was `null` for
   *every* project, so every row read `"{agent} activity"`. The second is upstream — see
   the event-name section below.
5. **Two bugs were the same shape as slice 1's finding 3: one branch handling a case its
   sibling silently drops.** `ingest`'s commit arm required `Some`/`Some` while the agent
   arm treated `None` as older than any `Some` (a repo's first-ever commit emitted
   nothing); `feed_block_lines` padded to full height in its empty branch but not its
   populated one. Neither was caught by a green suite. Standing habit: when a function has
   parallel arms, diff the arms against each other, not just against the spec.
6. **`MM-DD` fixed the ambiguity but not the readability.** Tinting the time field by
   recency (`FRESH` today, `COLD` earlier) makes the today/earlier boundary a single
   visual break instead of eight small reading tasks. A separator row was rejected
   because the feed's row budget is scarce (`FEED_MIN_ROWS` is 4).
7. **The test written for the new colour found an older bug in the same function.**
   Checking the stamp survives a narrow pane meant asserting every line fits `width` at
   degenerate widths — and the ` ACTIVITY` label had never been clipped at all, overrunning
   any pane narrower than nine columns. A test aimed at a new feature is a good moment to
   widen the range the old ones were checked over.
8. **A speculative reservation beat the feature it was reserving against.**
   `FEED_MAX_ROWS` capped the block at 12 rows so `SPACE-2`/`SPACE-3` would still have
   surplus "later". Neither exists; on a terminal with ~30 spare rows the cap left two
   thirds of them blank. Replaced with a content-aware bound. Transferable: do not ration
   a shipped feature on behalf of an unbuilt one.
9. **Two guards were protecting rows that no section could have used.** The feed refused
   to draw whenever anything was truncated/skipped, and in the compact tier — both
   redundant given how the budget is spent. A guard justified by a priority argument still
   needs checking against the arithmetic.
10. **A gate that rejects correct work costs as much as one that passes broken work.**
    This slice's verify script twice failed on pre-existing clippy debt unrelated to the
    change, and `cargo clippy` only emits diagnostics for crates it recompiles, so a
    cached run passes by doing no work. Fixed by forcing a re-lint scoped to the files
    this slice authored outright.

### The feed's event name — fixed in the sensor, not in the fold

An earlier draft of finding 4 proposed fixing the missing event name in `swab`'s
newest-wins fold: prefer a signal that *has* a name when timestamps are close. That would
have worked and was the wrong fix, recorded because the reasoning generalises.

**The fold fix inherits a two-word vocabulary.** `installer.py`'s `HOOK_EVENTS` registers
`swab-hook` on exactly `PreToolUse` and `Stop`. A successful fold fix turns
`"claude-code activity"` into `"claude-code pre tool use"` — internal jargon on a glance
surface. Widening `HOOK_EVENTS` means rewriting every user's `settings.json`. A fix whose
value depends on a prerequisite that expensive isn't the cheap fix it appears to be.

**The name was also never durable.** `events::read_and_compact` truncates
`events.ndjson` every tick, so even a hook signal that won its fold reverted to `null` on
the next scan — a field that flickers reads as a bug, not a limitation.

**So: derive the name where the winning signal already comes from.** `sensors/claude.rs`
already reads each transcript's tail for `cwd`/`sessionId`; `event_name_for` now also
takes the last recognized conversational record's name off that same pass. No fold
change, no skew between a name and its timestamp, and it's re-derived from a durable file
every tick.

Five things worth carrying forward:

1. **The allowlist is the design, not a safety net.** Real transcript tails are full of
   record types this sensor does not model. "Take the last record's `type`" would render
   `"bridge session"` into the feed. Unrecognised types fall back to `"activity"` — a
   format change degrades to boring, never to garbage.
2. **First real use immediately revised the derivation, and the revision is the whole
   value of the feature.** Taking the last recognized record, full stop, produced
   `"assistant message"` 19 of 25 times, because a turn is shaped "run a tool, then say
   what it found" — the agent's closing prose is almost always last. Names now come with
   a *strength*: prose is weak (fills an empty slot, never displaces), a tool name or user
   prompt is strong. `assistant message` then disappeared entirely. Transferable: "last
   wins" is a plausible default that quietly loses to whatever your data happens to end
   with — check the resulting distribution, not just the rule.
3. **The `tool_result` exclusion is what makes it readable.** Every `tool_use` is followed
   by a `user` record echoing its result; honouring those would make a live session read
   `"tool result"` on nearly every tick.
4. **The real-data probe found what fixtures could not, again.** `swab/examples/probe.rs`
   runs one sensor against the real `~/.claude/projects` without writing
   `projects.json` — safe to run repeatedly mid-change. Look for a non-destructive probe
   entry point before reaching for the real writer.
5. **The hook's name had to be made to lose, which is the fold change the earlier draft
   proposed — arriving for the opposite reason.** Once tool names existed, an actively-
   running project flickered between `claude-code grep` and `claude-code pre tool use`.
   `scan.rs` now lets the transcript's name replace whatever won, keyed on the *source*
   (not a list of known lifecycle names, which Claude Code could change any time), while
   the hook's timestamp still wins as the more precise liveness clock.

Deliberately not done: widening `HOOK_EVENTS` further. A hook-supplied name always loses
for Claude Code projects now, so more registered events would add names that can never be
seen. The one thing that would change this — `Notification`/`PermissionRequest` carrying
"the agent is waiting on you" — is a real capability no transcript record expresses, and
became `MECH-5` (slice 6) rather than a side effect of lengthening a list.

`sensors/copilot.rs` still hardcodes `event: None` and has no event source at all, so its
projects keep reading `"copilot activity"` — untouched scope, not an oversight.

---

## Slice 5 — `SPACE-5`, the collapsed tab strip

Raised from real use, with a screenshot: collapsing `IN FLIGHT`, `STALE` and `COLD` to
get a "what's running" view still spent **9 rows** — rule, label, rule, three times — to
say almost nothing. `STALE 32` does not need three rows of screen to communicate 32.
Consecutive collapsed sections now share one line, bracketed by the same two rules a
single header gets: 3 rows for the whole run.

Three things made it work rather than merely fit:

- **Each entry stays a selection stop.** The temptation is to render a summary; that
  would break the load-bearing rule that a collapsed section must be reachable, since
  `STALE`/`COLD` ship collapsed and there would be no way to reopen them. `j`/`k` walks
  the strip and the selected entry takes the usual highlight, which is also what makes it
  read as *tabs* rather than a caption.
- **Runs group in place**, not "all collapsed sections gathered into one strip."
  Gathering is simpler and lets a section jump out of its normal position when the
  collapsed ones aren't contiguous — a worse surprise than the extra branch costs.
- **The label rule is shared** with the full-width header, so a collapsed `RUNNING`
  cannot say something an expanded one wouldn't.

Implemented in `dashboard::collapsed_strip_line` and the `joins_open_strip` branch in
`plan_layout`.

---

## Slice 6 — `MECH-5`, "waiting on you"

Landed 2026-09-04. `projects.json` can now say *why* a project is silent, in the one case
where the answer is "because it is waiting for you." `swab scan` sets
`agent.waiting_since` from Claude Code's `Notification`/`PermissionRequest` hooks, clears
it on the `PreToolUse`/`Stop` that means the human answered, and buckets a waiting
project `active` so it cannot decay out of view while still blocked. `SPEC.md` §3.2/§4.6
have the behaviour.

Why this needed a new signal rather than tuning thresholds: every agent state is derived
from *silence* — `agent_state_for_silence` maps seconds-since-last-event onto
Working/Recent/Idle, and that is the whole vocabulary. An agent blocked on a permission
prompt and an agent mid-thought produce the same observation — nothing — so a blocked run
decays exactly as if it had finished cleanly. Claude Code's `Notification` and
`PermissionRequest` hooks fire precisely when it needs a human; no transcript record
expresses this, so it's a genuine capability the transcript cannot supply. These two
events are also rare (unlike `PostToolUse`, which fires on every tool call), which is why
widening `HOOK_EVENTS` for them was worth it when it wasn't for general event-name
granularity (see slice 4's event-name section).

Eight things worth keeping:

1. **The signal is durable; the file it arrives in is not.** `events.ndjson` is truncated
   every scan, so "no waiting event this tick" is the **normal** condition of a project
   still blocked — a naive set-here/clear-there reading releases every latch one tick
   after setting it. The latch lives in `projects.json`, carried forward from the
   previous `Radar`, exactly like `agent_activity`'s ring. Before building on an event
   source, check its retention, not just its contents.
2. **Absent and `false` had to stay different answers.** A `WaitingDeltas` entry means "a
   relevant event was seen this tick"; its absence means "no news, keep what you had."
   Collapsing those into a `bool` makes an ordinary liveness tick indistinguishable from
   the human answering, and the feature dies silently. A three-valued question wearing a
   two-valued type's clothes.
3. **File order beats timestamp order here, and the existing tie rule was the wrong one
   to copy.** `format_at` writes whole seconds, so a `Notification` and the `PreToolUse`
   that answers it routinely land in the same second; `O_APPEND` order is the only
   truthful chronology. The signal fold two lines away resolves ties by *keeping the
   earlier* entry, which here would mean a permission prompt outliving its own answer.
   Two accumulators over one loop, two opposite tie conventions — both correct, neither
   transferable.
4. **The release rule needed a backstop the original sketch didn't ask for.**
   Hook-clearing is correct and insufficient: a killed session, a closed terminal, or a
   sleeping machine simply never sends one. `WAITING_MAX_LATCH_S` (3h, matching
   `RUNNING_ATTENTION_CEILING_S`) is what makes the pin safe to grant. A repeated set
   deliberately does **not** restart the clock, or a re-firing notification would defeat
   its own expiry.
5. **Fixing the sort in `petri` alone would have been the wrong half of the fix.**
   `running_membership` filters on `status_bucket == Active`, so making `swab`'s
   `status_bucket` honour the latch fixes it for every reader (`petripy`, the menubar,
   `swab list`) at once. A capability added to the schema that only one frontend honours
   is not a schema capability.
6. **`swab doctor` had the same bug one layer up, and it was the more dangerous half.**
   Its hook check was a substring test that reports "ok: hook" on any machine registered
   on only *some* of the events — after this slice, every machine that ever ran
   `install`. Now checked per event, with the missing names and the fix in the message.
   When you widen a list something is installed from, grep for every other place that
   answers "is it installed?"
7. **Growing `HOOK_EVENTS` was a no-op until the installer's idempotency changed shape.**
   `add_hook_entries` short-circuited on the marker appearing *anywhere* in
   `settings.json`, so an already-installed machine would have been told "already
   installed" and never received the two new events. The per-event check is what makes
   the list growable at all.
8. **A test that re-implements the branch it is testing proves nothing.** The copilot
   guard was first "tested" by copying its `match` into the test body and asserting on
   the copy — green, meaningless, and it would survive deleting the guard entirely.
   Extracting `waiting_for_root` so the test could call the real thing was the fix. When a
   test is awkward to reach, the shape of the code is usually the thing to change.

Left open, deliberately:

- **The menubar does not show it.** `menubar.py` selects its live list on
  `agent.state == "working"`, and a waiting project is by definition not working. Not
  smuggled in; `schema.py` carries the field so it's a small change later.
- **The end-to-end path has not been observed on a real blocked session.** Every layer is
  tested against real fixtures, and both hook names are verified live in this machine's
  `settings.json` — but nobody has watched a real permission prompt light up the
  Dashboard yet, since that needs a re-run of `install` plus a session that actually
  blocks.

---

## Slice 7 — `ACT-2`'s `f`/`y`/`s`/`?`, delegated to a local model

Landed 2026-09-06, issue [#27](https://github.com/JKrag/petridish/issues/27). Retrospective
prediction 1 held again: three of the four keys were cheap `tools::registry()` table
rows exactly because slice 1 built the mechanism. `f` (reveal in Finder) and `s`
(rescan now) are both single `Action` entries — `s` needed no new reload logic at all,
since `petri` already polls `projects.json`'s mtime every second and firing `swab
scan` is the whole job. `y` (yank to clipboard) and `?` (help popup) are the two
genuinely new pieces: `y` spawns `pbcopy` directly rather than going through the
registry, since it needs no terminal hand-off and isn't a choice between programs;
`?` is `MECH-1`'s second customer, a pure-content popup whose action-key list is
generated from `tools::registry()` rather than hand-typed.

This slice was built by delegating each of the four keys to a local model (oMLX,
`delegate-afk`) rather than writing them directly, with the orchestrator reviewing
and committing each round. Two findings worth keeping:

1. **Model tier tracked task shape, not task size.** The smallest tier (a ~4B model)
   failed all three attempts at even the simplest task — a single `Action` entry
   inserted mid-`Vec` — either rewriting the entire 600-line test file, mangling
   existing code while trying to insert around it, or giving up outright once it
   could not make its edit tool's exact-string match land. A mid-sized tier (~9B)
   got every one of its four rounds right on the first try, including the two
   structurally harder tasks that touched the event loop and needed a new PTY test
   file. The largest tier (~35B) was never actually exercised on a task, because it
   hit a memory-ceiling rejection before generating anything — the machine did not
   have headroom for a 35B model plus a long, detailed prompt at the same time.
2. **`make check` cannot catch a widget that's built but never rendered.** The help
   popup's `Block` (border + title) was constructed and had `.inner()` called on it
   for layout, but the line actually painting it — `frame.render_widget(block,
   popup)` — was missing. This compiles clean and passes clippy, since the `Block`
   value is genuinely used (for its `.inner()` geometry), just never drawn. Caught
   by reading the diff against `picker.rs`'s render function line-by-line, not by
   any automated gate. A rendering module that builds a widget and computes its
   inner area is worth a specific "did this actually get painted" check in review.

The stretch `c` (`cd` here on exit) was intentionally left out of this slice — it
needs a shell-wrapper design (petri hands a path back to the calling shell, `broot`/
`zoxide`-style) settled first, and that design is written up as a comment on #27
rather than built.

---

## Retrospective

Two predictions from the original brainstorm, worth checking against what actually
happened:

1. **"`MECH-2` plus one action key" would make every other `ACT-*` entry a table row.**
   Held exactly — slice 1 built it, and the remaining `ACT-2` keys are cheap precisely
   because of it.
2. **The activity feed (`SPACE-1`) would be the best demo material.** The payoff
   prediction held; the mechanism prediction (that `events.ndjson` could back it) did
   not — see slice 4's event-name section.
