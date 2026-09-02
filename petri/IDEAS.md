# `petri` — idea backlog

**Status: brainstorm, not spec.** Nothing here is decided. `petri/SPEC.md` is the
authoritative document for what `petri` *is*; this file is the pool of candidate
directions we draw from when deciding what it becomes next. An idea moving from here
into `SPEC.md` is the moment it stops being an idea.

Every entry has a stable ID (`ACT-3`, `SPACE-1`, …) so it can be referenced in
conversation, commits and issues without re-describing it. **IDs are never reused or
renumbered** — a dropped idea keeps its ID and gets marked `[dropped]` with the reason,
because "why didn't we do X" is worth as much as the ideas we kept.

**A built idea is marked `**DONE**` with a pointer to the code, not deleted.** Deleting
it would break the cross-references that open ideas make to it (`SURF-3` leans on
`MECH-2`, `SURF-6` on `MECH-1`, `ACT-6` on `ACT-1`), and the entry usually carries the
*reasoning* the code can only imply. Where an entry turned out to be only partly built,
the marker says which part — `ACT-2` and `ACT-9` are both live in exactly that way, and
those remainders are real work, not bookkeeping. `SPEC.md` is where the built behaviour
is specified; this file is where its motivation lives.

Origin: brainstorm session 2026-09-01, on the `petri-dashboard-redesign` branch, prompted
by two complaints about the then-current build — it wastes vertical space on tall
terminals, and the two screens are so close to each other that neither feels like it does
anything.

---

## 0. The framing these ideas came out of

Two observations shaped everything below.

**FRAME-1 — Most "make it interactive" ideas reduce to one primitive: suspend-and-exec.**
Open in editor, jump to a git graph, attach to tmux, open the GitHub URL — nearly all of
them are "hand the terminal to another program, take it back when it exits." Build that
once and the rest is a table of entries. See `MECH-2`.

**FRAME-2 — petri is a *router*, not a reimplementation of every tool it points at.**
Great TUIs already exist for git history (`serie`, `lazygit`, `gitui`, `tig`) and for token
usage. petri's unique asset is that it knows *which project needs you right now*; its job
is to notice that and hand you to the right tool for it, not to grow a mediocre in-house
version of each. This is also the sharpest one-line pitch for a public release: not
"another agent dashboard" but "the launcher for your fleet."

**Screen division of labour that follows from FRAME-2:**

- **Dashboard** — aggregation, glance, no scroll. Answers *"does anything need me?"*
- **Browser** — complete, scrollable, actionable. Answers *"show me everything, and let
  me act on it."*

The two screens feel redundant today because the Browser has the *detail* half of that
and none of the *act* half. The lever is **actions in the Browser, aggregation on the
Dashboard** — not more metrics on both.

---

## 1. Mechanisms (the enabling primitives)

### MECH-1 — Popup / overlay frames
**DONE** (slice 1) — `petri/src/picker.rs`'s `render`. Kept because `SURF-6` and the
`?` help popup still draw on it.

**Feasibility: trivial. Not a terminal-capability question at all.** Render `Clear` into a
centered `Rect`, then the content block on top, last in the draw call. It is ratatui
painting its own cells; nothing is negotiated with the terminal emulator. Anything
renderable as a screen is renderable as a popup.

Customers: help screen (`?`), confirmation dialogs, quota detail, per-project detail
in place on the Dashboard (which is `SPEC.md` §7's deferred "inline card expansion",
now with a mechanism).

### MECH-2 — Suspend-and-exec (hand the terminal to a child process)
**DONE** (slice 1) — `petri/src/exec.rs`. Kept because `SURF-3` and the unbound rows of
`ACT-2` still draw on it. One correction from building it: the `terminal.clear()` in the
sketch below is *wrong* on ratatui 0.30 — see `§7` finding 2.

**Feasibility: easy, ~30 lines, and it is the *right* answer rather than a compromise** —
a git-graph browser wants the whole screen, not a 40-column pane. `lib.rs`'s existing
setup/teardown pair is already the two halves of it.

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

Three failure modes worth writing into the spec when this lands:

1. **`.status()`, never `.output()`.** `output()` captures stdio, so the child renders
   into a pipe: you see nothing and it looks like a hang.
2. **`terminal.clear()` on the way back is mandatory.** ratatui diffs against a cached
   previous buffer; without an explicit clear it believes cells the child overwrote are
   still correct and you get a half-painted frame.
3. **Restore on the child's failure path too** (binary missing, child panics), or a
   missing tool leaves the user in raw mode with no shell echo.

Note `petri` does not currently enable mouse capture (`lib.rs`), so there is no mouse
mode to tear down and restore. If mouse support is ever added, it joins both halves.

### MECH-3 — Spawn-and-detach (for GUI targets)
**DONE** (slice 1) — `petri/src/exec.rs`, and `ExecMode::{Terminal,Background}` in
`tools.rs` is the per-entry mode this asked for.

Distinct from `MECH-2` and needed alongside it. `code $folder`, `open -a`, a browser URL:
these must **not** take the terminal and must **not** be waited on. Different call shape
(spawn, null stdio, don't wait, don't tear down the TUI), so the external-tool registry
(`ACT-1`) needs a per-entry mode: `Terminal` (suspend, wait) vs `Background`.

### MECH-4 — Embedding a live child *inside* a ratatui pane
**Feasibility: real, but expensive — recommended against for now.** `portable-pty`
(already a dev-dependency here, so it builds on this machine) plus a vt-parsing widget
such as `tui-term`. You become a terminal-emulator host: resize propagation, key encoding,
mouse forwarding, alternate-screen semantics inside a sub-rect.

That cost is justified only for something you want *persistently alongside* the UI — a
live log tail in a corner — not for "press `g`, look at the graph, come back," which
`MECH-2` covers at a fraction of the complexity.

---

## 2. Actions — making the Browser *do* something (`ACT-*`)

### ACT-1 — An external-tool registry, not hardcoded keys
**DONE** (slice 1) — `petri/src/tools.rs`. The `petri.toml` override half is real but
narrow: the `[tools]` table stores *which program* per action, not yet a full argv
override. Kept because `ACT-5`/`ACT-6`/`SURF-4` all extend it.

Each action is data: name, key, probe (how to tell it is available), argv template, exec
mode (`MECH-3`), and an ordered fallback chain. Three payoffs: the footer can honestly
advertise only what is bound (required by `SPEC.md` §3.1), the fallback chain becomes
declarative, and the whole table is overridable in `petri.toml` — which is the concrete
form `FRAME-2` takes in the code.

### ACT-2 — Candidate action keys for the Browser
**PARTLY DONE** (slice 1 + 2) — three of nine bound. The remaining six are the live part
of this entry; each is now a registry row (`ACT-1`) rather than new plumbing.

| key | action | mechanism | status |
|-----|--------|-----------|--------|
| `o` | open the project's remote in a web browser | `MECH-3` — spawn-and-detach, same as the GUI editors; `github_url` is already in the schema | **done** (`O` re-picks) |
| `g` | git history / graph | `MECH-2`, fallback chain — see `ACT-3` | **done** (`G` re-picks) |
| `e` | open in editor | `MECH-2` or `MECH-3` depending on target — see `ACT-4` | **done** (`E` re-picks) |
| `t` | attach to the agent's tmux session | `MECH-2` (`attach`), or `switch-client` when already inside tmux | open — see `SURF-3` |
| `f` | reveal in Finder | `MECH-3`, `open <path>` | open |
| `y` | yank path to clipboard | for pasting into another terminal | open — the one row that is *not* a registry entry; no child process involved |
| `c` | `cd` here on exit | print the path for a shell wrapper to consume; petri becomes a navigator | open — needs a shell-wrapper story first |
| `s` | rescan now | invoke `swab scan` and refresh — directly answers "it's just a viewer" | open |
| `?` | help popup | `MECH-1`'s first customer | open — and `MECH-1` is built, so this is now pure content |

### ACT-3 — Git history with a graceful fallback chain
**DONE** (slice 1) — `registry()`'s `gitlog` action, pinned pager and all. The measured
`less -F` caveat below is implemented, not just noted.

`serie` → `lazygit` → `gitui` → `tig` → **plain `git log --graph --oneline --decorate
--all`**. The last one is not a degraded consolation prize: git pages through `less` by
itself when stdout is a tty (its default pager config already handles color and quits on
`q`), so the *interaction* is the same as the fancy tools — full screen, `q` returns to
petri. That means `g` can always be bound and always advertised, and tool detection only
decides *which* graph you get, not whether the key exists.

One measured caveat on the fallback: git's default pager is `less -F -X`, and `-F` makes
less print-and-exit when the output fits one screen — so a short history would flash past
instead of behaving like a TUI. The fallback should therefore pin its own pager for that
one invocation (`git -c core.pager='less -R' log --graph …`) rather than inheriting
whatever the user's config happens to be, so the interaction is uniform across every
entry in the chain.

### ACT-4 — "Open in editor" and the folder-vs-file problem
**DONE** (slice 1) — the resolution order below is `begin_action`'s editor chain, and
the folder-capable candidates are `registry()`'s `edit` action. **One caveat kept
deliberately: the `Background` path has never been exercised against a real GUI editor
on a desktop** — only unit-tested. Until someone presses `e` on a real machine with
`code` installed, treat that half as unverified.

The real question is not detection but **what we hand the editor: a directory.**

- Multi-file / GUI editors take a folder natively: `code`, `cursor`, `zed`, `subl`,
  `idea`, `windsurf`. These are `MECH-3` (don't block the terminal).
- Most terminal editors handle a directory fine too: `vim`/`nvim` open netrw, `hx` opens
  its file picker, `emacs` opens dired. These are `MECH-2`.
- The genuinely awkward case is small: `nano` and friends, which have no directory
  concept.

Measured on this machine, the more common situation turns out to be different from the one
we were worried about: **`$EDITOR` and `$VISUAL` are both unset**, while `code` is on
`PATH`. So the env-var chain frequently yields *nothing at all*, and a probe-based default
(prefer a known folder-capable editor found on `PATH`) is doing more work in practice than
the nano edge case is.

So it is a real but *narrow* problem, and it does not need clever inference. Resolution
order: an explicit `editor` entry in `petri.toml` → `$VISUAL` → `$EDITOR` → a
best-available probe. The config override is what turns the awkward case into a
non-problem — a `nano` user sets the key once. The registry (`ACT-1`) carries whether a
given editor is `Terminal` or `Background` mode.

### ACT-5 — Fleet-level actions on the Dashboard
The Dashboard gets actions too, but *different* ones — fleet-scoped, not project-scoped.
"Attach to the run that needs me" (`Enter` on the top RUNNING card goes straight to tmux,
skipping the Browser) is the archetype. This is a large part of what stops the two screens
feeling like duplicates.

### ACT-6 — Multi-select and bulk action
Mark several projects, then apply one action to all of them (rescan, run a command in
each). Depends on `ACT-1`; only interesting once single-target actions exist.

### ACT-7 — Detail pane shows affordances, not just facts
Follows from `FRAME-2`: if petri is a router, the Browser's detail pane should be mostly
*"here is what you can do with this project"* rather than a static fact sheet. This
changes the pane's whole design and should be decided before investing more in its
current layout.

### ACT-8 — First-run tool picker popup
**DONE** (slice 1) — `petri/src/picker.rs` + `[tools]` in `prefs.rs`. Kept because
`ACT-11` refines it and the two must be read together.

When an action has several plausible targets on this machine (four git TUIs installed; or
`code`, `nvim` and `vim` all on `PATH`), don't guess and don't make the user find the
config file first. On the **first** invocation of that action, open a `MECH-1` popup
listing the probed candidates, plus an `Other — specify path…` entry, with a footnote
saying where the choice is stored so it can be changed later. Persist the answer to
`petri.toml` and never ask again.

This turns `ACT-1`'s registry from a static table into a **resolver with state**, which is
the substantive design consequence and the reason to decide it early rather than bolt it
on. Details worth settling with it:

- **Only ask when the choice is genuinely ambiguous.** Zero candidates → go straight to
  `Other — specify path…`. Exactly one → just use it, silently; a modal to confirm the
  only option is pure friction.
- **Order candidates opinionatedly**, best-guess first, so `Enter` does the sane thing and
  the popup costs one keystroke.
- **Offer a re-pick key** rather than relying on the footnote. "Go edit the TOML" is a fine
  escape hatch but a poor primary path; a shifted variant of the action key (`G` re-picks
  what `g` runs) is better, and the popup's footnote can mention both. See `ACT-11`, which
  refines what that key *is* — it turned out to be a one-off launcher first and a settings
  dialog second.
- **Re-ask when the stored choice disappears** from `PATH` — a removed tool should reopen
  the picker, not produce a failed exec.
- **Tests must never see the popup.** The PTY layer (`SPEC.md` §8, already the flakiest)
  would hang on a modal it doesn't know to answer. Pre-seed the preferences file in the
  fixture, so "unconfigured" is an explicit state the tests choose to enter. Note `ACT-11`
  *inverts* this for the re-pick popup specifically: there the popup is the feature, so its
  test has to open it deliberately and answer it.

### ACT-9 — Action availability has two independent axes
**DONE** (slice 1) — `Resolution::{NoTool,NoTarget}` in `tools.rs`. The second axis
currently reads as a transient notice rather than the dimmed affordance this asked for;
that part is **still open**.

Worth separating in the registry, because they fail differently:

- **Tool availability (per machine)** — is `serie` installed? Resolved by probing, and by
  fallback chains (`ACT-3`) or the picker (`ACT-8`).
- **Target availability (per project)** — this project has no `github_url`, or no tmux
  session, so `o`/`t` have nothing to act on even though the tooling is fine.

The first decides whether a key is *bound at all*; the second decides whether it is live
for the currently selected row, and should read as a disabled/dimmed affordance rather
than a key that silently does nothing.

### ACT-10 — The `/` filter query is never shown on screen
**DONE** (slice 3) — `browser::filter_chip_spans`, plus `BrowserState.filter_input`
(the mode flag moved out of `lib.rs`'s event loop so `render` can see it). Kept
because the reason it went in the *header* rather than the footer is a constraint
that outlives the fix — see `§9`.

Found while gating the action keys: `BrowserState.filter_query` is stored and
applied, but `browser::render` never displays it. The only visible evidence that a
filter is active is that the list got shorter — so a user who filters, looks away,
and looks back has no way to tell a filtered list from a fleet that went quiet.
Small fix, real confusion, and it also costs the PTY layer a natural assertion
target. Not fixed with the action keys, because it is a separate change and the
commit was already large.

### ACT-11 — The re-pick key is a one-off launcher, not a settings dialog
**DONE** (slice 2) — `picker::Mode`, `Outcome::Chosen { persist }`, `begin_repick` in
`lib.rs`.

A refinement of `ACT-8`'s re-pick bullet, from the daily-use case that motivates it: *"I use
`serie` as my everyday git viewer, but just this once I want `lazygit`."* That is not a
request to change a default — it is a request to bypass one. If the shifted key were purely
"reconfigure this action", using it would silently cost you your real default every time you
wanted a one-off, and you would then have to re-pick back.

So the popup opened by the shifted key carries **two verbs, not one**:

| key | meaning |
|-----|---------|
| `Enter` | launch the highlighted tool **once**; stored default untouched |
| `D` | make the highlighted tool the **new default**, and launch it |
| `Esc` | cancel; launch nothing, change nothing |

`Other — specify path…` needs care, because inside the text field `D` is a literal
character and cannot also be a verb. Rather than reach for a modifier (`Ctrl-D`) or leave a
dead end where a hand-typed path can only ever be one-off, **the field inherits the verb you
opened it with**: `Enter` on the `Other` row opens it in run-once flavour, `D` on the `Other`
row opens it in set-default flavour, and `Enter` in the field commits with whichever it is.
The footer names the live one, so the user never sees a key advertised that the current mode
does not honour.

The first-run popup (`ACT-8` proper) keeps its single verb — there is no default yet to
preserve, so `Enter` there stores *and* launches as before. Same widget, two modes; the mode
is what `Enter` means.

Three consequences worth pinning:

- **`Shift`+`Enter` is not available**, however natural it reads. Terminals do not report it
  distinguishably from plain `Enter` without the kitty keyboard protocol, and `petri` pushes
  no `KeyboardEnhancementFlags` (it does not even enable mouse capture — see `MECH-2`'s
  note). Turning them on to win one binding would change key decoding globally and put the
  flakiest layer in the repo at risk. `D` is a plain, portable key that the footer can
  honestly advertise, which is what `SPEC.md` §3.1 asks for anyway.
- **The shifted key must not route through `Resolution::Ambiguous`.** That variant only
  appears when the choice is *unresolved*; the whole point here is that it is resolved and
  the user wants to override it anyway. Re-pick needs its own entry point that lists every
  installed candidate — fallbacks included — regardless of ambiguity, and falls through to
  `Other — specify path…` when nothing at all is installed.
- **Bind the shifted variant for every registry action, not just `g`.** `O` looks thin today
  (one candidate, `open`, handing off to the system default browser) but the `Other` row is
  exactly how someone pins a specific browser, and a rule that holds for every key stays
  true as the registry grows. A per-action exception list is the thing to avoid.

---

## 3. Reclaiming vertical space (`SPACE-*`)

**Diagnosis first — this is not a bug, it is three current rules with nothing to
counterbalance them.** `COMPACT_TIER_MAX_CONTENT_ROWS` is a *floor* (go compact below 16
rows); "truncate, do not scroll" caps content from above; STALE/COLD ship collapsed.
Nothing in the design ever *grows* to consume surplus height, so on a tall terminal every
section emits its fixed content and the remainder is blank by construction.

### SPACE-1 — Fill the slack with a live fleet event feed
`events.ndjson` already exists (written by `swab-hook`). A scrolling activity feed —
`14:22  project-radar · agent stop · 3 files` — fills leftover height with something
genuinely new, makes the Dashboard feel alive rather than static, and is agent-agnostic by
construction (it shows whatever wrote the event, not "Claude"). Highest ratio of visible
payoff to effort on this list, and the best candidate for a demo GIF.

### SPACE-2 — Auto-expand STALE/COLD into leftover room
The collapse defaults were space rationing for small terminals, never a stated preference.
With 20 spare rows, spend them; re-collapse when the room goes away. Must not fight a
user's explicit toggle.

### SPACE-3 — A third density tier *above* roomy
Symmetric to the existing compact/roomy switch: on tall terminals, more sparkline history,
more fields per card, bigger cards.

### SPACE-4 — Make "truncate, do not scroll" height-conditional
The rule exists so a glance surface can't be scrolled into a state where it hides the
alert — a risk that is really about *small* terminals. Truncating with blank space below
is the rule failing on its own terms. Worth revisiting rather than treating as settled.

---

## 4. New surfaces (`SURF-*`)

### SURF-1 — A timeline / history screen
`events.ndjson` again: what happened across the fleet today, as one horizontal swimlane
per project. Nobody else can show this, because nobody else is collecting fleet-wide agent
events.

### SURF-2 — Native notifications on state transitions
petri already knows when a run has gone silent past a threshold. A macOS notification on
transition (`osascript`, no new dependencies) makes petri useful when it is *not* on
screen — which is the entire point of an ambient monitor.

### SURF-3 — Session manager for tmux / zellij
Given projects and their sessions, "give me a session for this project, creating it if it
doesn't exist" is a small amount of shell-out and is genuinely daily-useful. Natural
extension of `ACT-2`'s `t`.

### SURF-4 — Quota / token pane, plus a key to a dedicated tool
`SPEC.md` §7 already lists quota bars as the most likely post-v1 addition. `FRAME-2` says:
ship a minimal in-house bar *and* bind a key to whichever token TUI the user prefers, via
the same registry.

### SURF-5 — `petri dash --once`
Also already deferred in `SPEC.md` §7: render one frame to an off-screen buffer, print,
exit — pipeable into a tmux status line. Cheap once the off-screen render exists, and a
good "look how it composes" line in a public announcement.

### SURF-6 — A "project detail" popup on the Dashboard
Expand a card in place to show bigger and better details, plus the action keys that are live for that project. Show bigger sparklines or other metrics, and make the Dashboard feel like a *dashboard* rather than a static list. Should allow the user to "focus" on a project without leaving the Dashboard, and to act on it without going to the Browser.

### SURF-7 — A minimal "run" screen for a single project
Basically a `petri --mini` mode: one project, one screen, no list. Split your terminal, so you have a small pane in the corner, run petri --mini, and have a live view of the run currently on your screen, i.e. in this terminal window/tab. This is a natural extension of `SURF-5` and `SURF-6`, and it is a good candidate for a demo GIF.

---

## 5. Constraints any of these must respect

Collected here because they are easy to trip over while building the above.

- **The footer may only advertise keys that are actually bound** (`SPEC.md` §3.1). A key
  whose tool isn't installed must be resolved at the registry level (`ACT-1`/`ACT-3`), not
  by letting the footer lie.
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

## 6. Where the leverage is next

Not a roadmap. The original two suggestions are recorded here as struck through rather
than deleted, because what they *predicted* turned out to be worth keeping.

1. ~~**`MECH-2` plus one action key.**~~ **Done — slice 1 (§7).** The prediction held
   exactly: "every other `ACT-*` entry becomes a table row once this works" is now
   literally true, and it is why `ACT-2`'s six open keys are cheap.
2. **`SPACE-1`, the event feed.** Still the highest payoff-to-effort item on the list,
   still independent of everything built so far, and still the best demo GIF. This is
   the obvious next slice.

Two cheaper things worth doing before or alongside it:

- ~~**`ACT-10`**~~ **Done — slice 3 (§9).** It was the only known *bug* on this
  list; everything remaining is a missing feature.
- **The rest of `ACT-2`.** `?` (help popup) and `s` (rescan) are now content-only:
  `MECH-1` and the registry already exist, so neither needs new plumbing.

---

## 7. Built so far

Slice 1 landed on 2026-09-01/02 (`e99cc8d`..`bd87b9b`). What is now real:

- **`MECH-1`** — `picker::render`, a centred popup over the Browser.
- **`MECH-2`/`MECH-3`** — `petri/src/exec.rs`. Suspend-and-exec and
  spawn-and-detach, plus `is_installed` as the single impure `PATH` probe.
- **`ACT-1`/`ACT-3`/`ACT-9`** — `petri/src/tools.rs`. The registry, the resolver,
  `Candidate::fallback`, and both availability axes.
- **`ACT-2`** — `o` (open remote), `g` (git history) and `e` (open in editor) bound
  in the Browser, with a transient notice for the cases nothing can satisfy.
- **`ACT-8`** — `petri/src/picker.rs` plus the `[tools]` table in `prefs.rs`.

Three findings from building it, kept because they will outlive the code:

1. **`Candidate::fallback` was not in the original design and the design was
   broken without it.** `git` is on every machine, so a fallback that counted
   toward ambiguity would have made the picker fire for every user, forever —
   invisible on a machine with four git TUIs installed, which is where it was
   designed.
2. **`Terminal::clear()` is the wrong way to come back from `MECH-2` on ratatui
   0.30.** It snapshots the cursor via a blocking DSR query, at the worst possible
   moment. `Terminal::resize` does the same invalidation with no round-trip. The
   universally-given advice is right about the *what* and wrong about the *how*.
3. **Adding a field to `Prefs` silently wiped the `[tools]` table on every screen
   switch**, because three `Prefs { .. }` literals in `lib.rs` each named the new
   field as empty. No unit test could have caught it; `prefs::save` was called
   correctly with exactly the struct it was handed. Fixed at the root by keeping
   the loaded prefs alive and mutating in place.

Still open from slice 1's own list: the `e` action has never
been exercised against a GUI editor on a real desktop — only its `Background`
mode is unit-tested.

## 8. Slice 2 — `ACT-11`, the re-pick key

Landed 2026-09-02 (`b65c6ec`, `1a3bdd1`). `G`/`O`/`E` open the picker in re-pick
mode: `Enter` runs the highlighted tool once, `D` makes it the default and runs
it, `Esc` does neither. Shifted keys are derived from `action.key`, so a new
registry entry gets one for free.

- **`tools.rs`** — `launch_for` (rule 2's body, extracted) and
  `repick_candidates` (every installed candidate, fallbacks included, no
  ambiguity test, no stored-choice lookup).
- **`picker.rs`** — `Mode::{FirstRun, Repick}`, `Outcome::Chosen { program,
  persist }`, and a footer that advertises only the verbs live in the current
  mode.
- **`s8_pty_repick.rs`** — inverts `ACT-8`'s "tests must never see the popup",
  because here the popup is the feature.

Two findings worth keeping:

1. **`Resolution::Ambiguous` was the wrong door, and it fails silently.**
   Re-pick's premise is that the choice IS resolved; `resolve` collapses that
   case to `Ready` and throws the candidate list away. Routing `G` through it
   would have produced an empty popup on precisely the machines the feature is
   for — and looked fine on a machine with no default stored. The PTY test seeds
   a resolving default so a regression back to `resolve` cannot pass.
2. **A parameter removed is worth more than a test added.** `run_action` used to
   re-read `prefs.tools`, which was correct only because every caller wrote
   prefs first. Rather than test that a one-off doesn't, `prefs` was removed from
   its signature entirely — now it *cannot* consult a stored answer, which no
   later refactor can quietly undo.

The build ran as an AFK delegation to the local model and is worth recording
honestly. The model produced `tools.rs` and its eight tests, and most of the
assertions were real ones — registry order and the `fallback` flag, not just
counts. But the round failed its gate, and the audit that followed found the
thing the gate could not: `launch_for`'s known-candidate test used `serie`,
which takes no arguments, so it proved args *survive* without ever proving
`{path}` is still *substituted*. The test name promised the coverage; the body
didn't have it. That is the transferable lesson for the next delegation — check
assertion bodies against the spec, because a test that passes and is named
correctly can still be under-asserting. The model also froze twice with an
undiagnosed hang, and the picker/wiring half was written directly after the job
escalated.

## 9. Slice 3 — `ACT-10`, the invisible filter

Landed 2026-09-02. The header now carries a filter chip whenever a query is
live: `/query` plus `<matched> of <total>`, bright with a block cursor while
you are typing, dim without one once `Enter` has closed the input.

- **`browser.rs`** — `filter_chip_spans`, and `BrowserState.filter_input`.
- **`lib.rs`** — the loop's local `in_filter_input` is gone; the flag lives on
  `BrowserState` instead.
- **`s8_filter_chip.rs`** (render) and **`s8_pty_filter.rs`** (keystrokes).

Four things worth keeping:

1. **The chip belongs in the header, not the footer.** Replacing the footer
   with a filter prompt is the obvious design and it is barred by a rule
   already on this list: the footer may only advertise bound keys (§5). A
   filter-mode footer would either drop the keymap or advertise `Esc clear`
   in normal mode, where `Esc` is deliberately a no-op. The header had room;
   the footer had a contract.
2. **The interesting state is the *closed* one.** While you type, the query is
   at least implied by the keys you just pressed. The state ACT-10 was
   actually about is after `Enter` — and it is the one a naive fix (draw the
   input line while the input is open) leaves exactly as broken as before.
   The two render states are asserted separately for that reason.
3. **The header cannot wrap, so the chip needed a width budget.** At 40
   columns an over-long query pushed `0 of 15` off the right edge — losing
   exactly the disambiguator the whole idea is about, at a geometry `SPEC.md`
   §9 names as a test target. The count is laid out first and the query is
   elided into the remainder. A "does it panic at 40×10" test passed happily
   while this was broken; asserting the count is present is what caught it.
4. **The mode flag had to move to be renderable.** `in_filter_input` was a
   local in `poll_loop`, which is why the chip could not exist: `render` takes
   `&BrowserState` and nothing else. Mirroring it would have been two sources
   of truth for one mode, and the render would eventually have disagreed with
   the keymap — the same shape as slice 1's finding 3.

One gap this made visible rather than caused: **`Backspace` is unbound in the
filter input.** A mistyped query can only be abandoned (`Esc`) and retyped, and
now that the query is on screen the dead key is obvious. Not fixed here — it is
a keymap change, not a display one, and `SPEC.md` §5 does not currently list it.
