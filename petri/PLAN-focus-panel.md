# Implementation plan — the focus panel (`SURF-8`)

Companion to `petri/PROPOSAL-focus-panel.md`, which is the *what* and *why*. This is the
*how*, broken into units small enough to delegate.

**Written to survive three different execution paths**, because the choice isn't made yet:

| Path | What changes |
|---|---|
| **AFK against the local model** | Copy a phase's task queue into `.afk/program.md`. §2's verify script is the global verify command; §3's protected-file list is not optional. |
| **Direct implementation** | Ignore the tier tags and the budget. The task boundaries are still the commit boundaries. |
| **Fan-out to Haiku agents** | §8's dependency graph says what can run in parallel. Each task's "mutable files" list is the conflict-avoidance contract. |

What does **not** change across the three: the scaffold phases (§4, §6) are **cloud work,
not delegate work** — see §1 for why that is a correctness requirement rather than a
preference.

---

## 1. The verification problem, stated up front

`make check` **passes on unwritten code.** A delegate told to "add a focus panel" can add an
empty module, run the gate, see green, and report success — the exact false-done the
`delegate-to-local` start-gate exists to prevent. TUI layout makes this worse than usual:
the failure mode is "renders something plausible but wrong," which no exit code catches.

So this plan inverts the order: **the tests are the spec, and they are written before the
implementation, by whoever is planning — not by whoever is implementing.** If the delegate
writes both, the tests describe whatever it happened to build, and the gate is theatre.

That gives a real metric:

> **failing tests in `petri`, lower is better, 0 required to pass.**

and it makes each task a ratchet: a round either reduces the failing count or gets reverted.

**Consequence for AFK:** the default keep/revert rule in `templates/program.md` (pass →
commit, fail → revert) is **wrong for this job** and must be overridden in `program.md`.
The whole suite will not be green until the phase's last task. Use §2's rule instead.

---

## 2. The verify command and the keep rule

Phase 0 creates this file. It is the global verify command for every AFK round.

```bash
#!/usr/bin/env bash
# petri/scripts/afk-verify.sh — prints the number of failing tests in `petri`.
#
# Prints a single integer on stdout and always exits 0, so the AFK loop's
# "verify command errored" escalation stays reserved for a genuinely broken
# environment (no cargo, no toolchain) rather than firing on a bad round.
#
# TWO separate build guards, because there are two distinct ways a build
# failure can make a bad round score as a good one:
#
#  1. Nothing builds at all -> zero "test result:" lines -> awk sums nothing
#     and prints 0, a perfect score for a round that broke everything.
#  2. *One* integration target fails to compile. `cargo test -p petri` builds
#     each file under tests/ as its own binary, so the others still run and
#     still print "test result:". The failing target's tests simply stop being
#     counted, and the score DROPS. This is the nastier of the two: it looks
#     like progress, and deleting a test file has the same signature.
#
# Guard 1 is `--no-run` (compiles every target, runs nothing). Guard 2 is the
# binary count, pinned at the end of each scaffold phase.
set -uo pipefail

# Pinned by the scaffold phase: how many test binaries must report a result.
# Update it in the same commit that adds or removes a test file, never in a
# commit that is trying to move the score.
EXPECTED_BINARIES=${EXPECTED_BINARIES:?set this from the scaffold phase}

if ! cargo test -p petri --no-run >/dev/null 2>&1; then
  echo 9999            # guard 1: something does not compile
  exit 0
fi

out=$(cargo test -p petri --no-fail-fast 2>&1)
seen=$(grep -c '^test result:' <<<"$out")

if [ "$seen" -lt "$EXPECTED_BINARIES" ]; then
  echo 9999            # guard 2: a target vanished from the run
  exit 0
fi

grep '^test result:' <<<"$out" \
  | awk '{for (i = 1; i <= NF; i++) if ($i == "failed;") s += $(i-1)} END {print s+0}'
```

`--no-run` costs one extra link pass per round and buys the guard that matters most; take
the trade. Note it also makes guard 2 nearly unreachable in practice — it is kept because
guard 2 is the one that catches a *deleted* test file, which compiles perfectly.

**Keep/revert rule (overrides the template default):**

- Let `BASE` = the score at the start of the round, `NEW` = the score after it.
- `NEW < BASE` → **commit**.
- `NEW >= BASE` → **revert** (`git restore . && git clean -fd`) and re-prompt.
- `NEW == 0` → the phase is done; run `make check` (workspace-wide) before the final commit
  and stop.

**Never let a round reduce the score by deleting or weakening a test.** This is the one
failure mode the ratchet is blind to, and it is why §3's protected list is load-bearing
rather than tidy. Add a cheap tripwire to the round loop:

```bash
git diff --stat "$BASE_SHA" -- petri/tests/ | tail -1   # must stay empty for T-tasks
```

---

## 3. Files: mutable and protected

**Protected for every task below** (a change to any of these is an automatic revert,
regardless of the score):

- `swab/`, `petridish-core/`, `petridish-cli/` — different crates, not this job.
- `petri/SPEC.md` — spec changes are a human decision (§9).
- `fixtures/` — shared across all four crates; a fixture edit silently rewrites other
  crates' tests.
- **every file already present under `petri/tests/`** at the start of the phase. Tasks add
  new test files only where explicitly listed; they never edit existing ones.
- `petri/src/theme.rs` — the palette is settled (`SPEC.md` §4.1). New colors are a design
  decision, not an implementation detail.

Per-task mutable lists are given below and are meant to be enforced, not advisory —
they double as the merge-conflict contract for the parallel fan-out in §8.

---

## 4. Phase A — scaffold (**cloud work, do not delegate**)

Deliverable: an API that compiles, plus a full failing test suite. Nothing implemented.

### A.1 `petri/src/focus.rs` — the API surface

The important structural decision, mirroring `dashboard.rs`'s existing `plan_layout`/
`DashPlan` idiom: **separate the "which rungs fit" arithmetic from the rendering**, so the
responsive behaviour is unit-testable at a pinned `Rect` with no `TestBackend` involved.

```rust
/// What the panel is pointed at. Not a bare `usize`: the Dashboard's cursor
/// visits section headers too (SPEC.md §3.2), and the empty selection must be
/// representable — same requirement §3.1 already places on the Browser's
/// detail pane.
pub enum FocusTarget {
    Project(usize),
    Section(StatusBucket, usize), // bucket + project count
    Nothing,
}

/// Ladder rungs, in priority order. `plan_rungs` returns a prefix of this.
pub enum Rung { Identity, Path, Git, Agent, LastEvent, Actions, Recent, Repo, Tree }

pub struct FocusCtx<'a> {
    pub radar: &'a Radar,
    pub target: FocusTarget,
    pub now: DateTime<Utc>,
    pub feed: Option<&'a FeedState>,
    pub prefs: &'a Prefs,
}

/// Pure: which rungs fit, given the room. No `Frame`, no `Buffer`.
pub fn plan_rungs(area: Rect, ctx: &FocusCtx) -> Vec<Rung>;

/// Renders the planned rungs. Every rung is its own `fn rung_*_lines`, so a
/// task can implement one without touching the others.
pub fn focus_lines(area: Rect, ctx: &FocusCtx) -> Vec<Line<'static>>;
```

Every function body is `unimplemented!()` at the end of this phase. Add
`#![allow(unused_variables)]` locally if clippy objects; remove it in the last task.

### A.2 Tests, all failing

- `petri/tests/s11_focus_plan.rs` — `plan_rungs` at every §10 size from the proposal
  (120×40, 100×30, 80×24, 60×20, 48×14, 36×10, 24×6, and below-floor), against all four
  fixtures including `hostile.json`. Assert the exact rung prefix. **These are the highest-
  value tests in the job** — they pin the responsive behaviour, which is the part a
  delegate is least likely to get right and least likely to notice getting wrong.
- `petri/tests/s11_focus_render.rs` — `TestBackend` structural snapshots per §8's
  "content that must appear, not byte-for-byte" rule. Pin a fixed `now`; §8's own
  requirement, and the silence strings are derived at render time.
- Add `focus.rs` to `glyph_portability.rs`'s module list **in this phase**. `SPEC.md` §4.2
  is explicit that a module added after the gate was written is exactly the one whose
  glyphs nobody has width-checked — `feed.rs` and `picker.rs` both slipped through.

### A.3 The verify script

Create `petri/scripts/afk-verify.sh` from §2, `chmod +x`, and confirm it prints a non-zero
integer (not `9999` — that means it doesn't compile).

### A.4 T0b, pulled forward out of Phase B

**Landed in Phase A, not Phase B** (see §5's T0b entry for what it is). The ratchet cannot
grade it: widening a visibility keyword makes no test go from fail to pass, so `NEW == BASE`
and §2's keep rule reverts it — taking with it the one task T1a–T4 all wait on. It is
mechanical, it is verifiable by `make check` plus a `git diff` review rather than by a score,
and doing it here also removes the three-way `dashboard.rs` claim §8 warns about.

**Phase A exit criteria:** `cargo test -p petri` compiles; `afk-verify.sh` prints N > 0;
`git status` clean after commit.

**Actual result (2026-09-08):** `afk-verify.sh` prints **39** — that is `BASE` for Phase B.
32 test binaries report, which is what `EXPECTED_BINARIES` is pinned to in the script.
`cargo test --workspace --no-fail-fast` fails in `s11_focus_plan.rs` and
`s11_focus_render.rs` and nowhere else, so §10's "confirm `make check` is green at BASE"
is satisfied in the only sense available: everything the job does not own is green.

---

## 5. Phase B — the ladder (delegable)

All tasks mutate `petri/src/focus.rs` only. (Not "plus their named test file": Phase A wrote
every test these tasks are graded by, and `petri/tests/` is protected — §3.)

**Every task below moves the score by at least one**, which the ratchet requires and which
does not come for free: `s11_focus_render.rs` deliberately asserts one rung per test rather
than several at once, because a test covering six rungs scores zero for five of the six
tasks that own them and gets their correct work reverted. Preserve that property if you add
tests.

### T0b — `pub(crate)` extraction, no behaviour change · `[easy]` · **DONE in Phase A**

> Pulled forward and landed as cloud work — see §4's A.4 for why the ratchet structurally
> cannot grade it. `ZONE_INDENT` and `ZONE_LABEL_WIDTH` were widened too, beyond the list
> below: a `ZoneRowSpec` cannot be constructed from outside `dashboard.rs` without them.

- **Mutable:** `petri/src/dashboard.rs`
- **Do:** widen these `dashboard.rs`-private helpers to `pub(crate)` so `focus.rs` can reuse
  them, changing nothing else: `zone_row` + `ZoneRowSpec`, `sparkline_glyphs`,
  `agent_sparkline_width_for`, `silence_tier_color`, `commit_ago`, `humanize_secs`,
  `abbreviate_home`. (`is_waiting` and `present::dirty_marker` are already public.)
- **Done when:** `make check` is green and `git diff` shows only visibility changes.
- **Why it is its own task:** it is the only thing T1a–T4 wait on, it is mechanically
  verifiable without any new test, and it touches the one file three other tasks also want.
  Folding it into T1 made a revert-loop there stall the whole phase.
- **Where these helpers actually live — verify, don't trust the doc.** `SPEC.md` §2 says
  `petridish-core`'s `present` module carries `silence_seconds`, `humanize_duration` and
  `is_stale`. **It does not** — `present` exports seven functions and none of those three
  are among them. The helpers you want are the `dashboard.rs`-private ones listed above.
  This is spec-lagging-code drift of the kind §1 of `SPEC.md` explicitly tells you to
  resolve in the code's favour; do **not** "fix" it by adding duplicates to core.

### T1a — `plan_rungs`: the responsive arithmetic · `[medium]`
- **Mutable:** `petri/src/focus.rs`
- **Do:** implement `plan_rungs` for every size in the proposal's §10 ladder.
- **Done when:** every test in `s11_focus_plan.rs` passes. No rendering yet.
- **Why separate:** this is the part a delegate is least likely to get right and least
  likely to notice getting wrong, and it is verifiable with zero rendering. Keep it alone so
  a revert here costs one cheap round rather than four renderers' worth of work.

### T1b — rungs R0–R3: identity, path, git, agent · `[medium]`
- **Depends on:** T0b, T1a
- **Mutable:** `petri/src/focus.rs`
- **Do:** the `Identity`/`Path`/`Git`/`Agent` renderers, reusing T0b's helpers, **plus the
  `FocusTarget::Section` and `FocusTarget::Nothing` empty states**.
- **Done when:** the `r0_`/`r1_`/`r2_`/`r3_`, waiting-latch and empty-state cases in
  `s11_focus_render.rs` pass.
- **Why the empty states moved here from T5:** they are `focus.rs`-only, so they stay
  conflict-free with the other rung tasks — and leaving them in Phase D made Phase B's own
  exit criterion ("`afk-verify.sh` prints 0") unreachable, since three tests in
  `s11_focus_render.rs` exercise them. An unattended loop would have spent its whole round
  budget discovering that.
- **Gotcha:** the waiting latch must be re-derived at render time via
  `waiting_latch_live(.., now)` — never read `agent.waiting_since.is_some()` directly. A
  stale state file otherwise pins a dead `▲` forever (`SPEC.md` §3.2).

### T2 — rungs R4 + R7: last event, repo · `[easy]`
- **Mutable:** `petri/src/focus.rs`
- **Do:** `last  {event} · {n files} · {HH:MM}` from `agent.last_event`/`last_event_at`;
  `repo  yours {age} · newest {age} · {url}` from `mine_last_commit_at` vs `last_commit_at`.
- **Gotcha:** `agent.last_event` is legitimately `None` — the sensor derives names from an
  allowlist and an unmodelled record type yields `None` *on purpose*. Fall back to
  `feed::agent_detail(p)`; a row reading `claude-code activity` is correct output, not a bug
  (`SPEC.md` §3.2). Do not "fix" it by widening the allowlist — that's in `swab`, protected.
- **Gotcha:** `mine_last_commit_at == last_commit_at` is the common case. Render one age,
  not two identical ones.

### T3 — rung R5: actions as affordances · `[medium]`
- **Mutable:** `petri/src/focus.rs`, `petri/src/tools.rs`
- **Do:** for each registry action, resolve against this project and render per the
  proposal's §4 table: live → `e edit nvim`; `NoTarget` → dimmed `o remote ─ no url`;
  `NoTool` → omitted entirely.
- **Gotcha:** `SPEC.md` §5 forbids advertising a key that does nothing — that is what makes
  `NoTool` an omission and `NoTarget` a dimmed entry, and the two must not be collapsed.
- **Gotcha:** resolution must not shell out per frame. If `Resolution` probes `PATH` on
  every call, cache it per reload, not per render — this runs at 2–5s poll cadence.
- **Non-color fallback required:** the `─` glyph and the words carry the disabled state.
  Dimming alone fails `NO_COLOR` and fails CVD readers.

### T4 — rung R6: per-project recent · `[medium]`
- **Mutable:** `petri/src/focus.rs`
- **Do:** filter `FeedState::events()` by `FeedEvent.project`, newest first, render with the
  existing stamp/tint rules (`FRESH` for today's clocks, `COLD` for earlier dates).
- **Gotcha:** reuse `FeedEvent::stamp`/`body_text` rather than reformatting. The date-vs-
  clock switch is already solved there and getting it wrong reads as a sorting bug.
- **Gotcha:** `feed: None` is a real state (`--mini` may start before two snapshots exist).
  Render the rung as absent, not as an empty box.

**Phase B exit:** `afk-verify.sh` prints 0; `make check` green; the panel is complete but
mounted nowhere.

---

## 6. Phase C — scaffold #2 (**cloud work, do not delegate**)

The mounts change key handling and app lifetime, so their tests need writing before the
implementations exist, same as Phase A.

- `petri/tests/s11_focus_mount.rs` — Dashboard state: `Space` on a header still toggles;
  `Space` on a project row opens the popup and **does not** move the cursor or toggle the
  section; `Esc` closes; the popup follows `j`/`k`; a header stop renders `FocusTarget::
  Section`. Pure-state, no terminal.
- `petri/tests/s12_mini_resolve.rs` — cwd → project resolution, the git-toplevel walk, the
  `--mini <PATH|NAME>` override, the not-a-project message, and re-resolution by name across
  a simulated re-sorted `Radar`.
- **Decide and write down the arg-parsing contract** — see T7's gotcha, it is a real trap.

### Actual result (2026-09-08)

Scaffold landed: `dashboard.rs` grows `focus_open` plus `focus_target`/`press_space`/
`close_focus`/`focus_placement`, and `lib.rs` grows `MiniTarget`/`CliArgs`/`parse_args`/
`MiniError`/`resolve_mini` — all `unimplemented!()`. `afk-verify.sh` prints **52** — that
is `BASE` for Phase D — from 23 failing tests in `s11_focus_mount.rs` and 29 in
`s12_mini_resolve.rs`, and nothing else in the workspace fails. `EXPECTED_BINARIES` is
pinned to **34** in the same commit that added the two test files, per §2.

**T5's geometry is score-graded after all**, which this section originally left to the
attended PTY pass. Six tests in `s11_focus_mount.rs` drive `focus_placement`, a pure
`Rect → Popup | FullScreen` function: §10's pressure-table rows that must be a popup and
those that must fall back full-screen, the ≤80%-in-both-axes rule, centring, the content
rect clearing the panel floor, and degenerate frames. Without them a delegate could pass
all 17 state tests with the overlay drawn wrong and legitimately report done — the state
tests cannot see geometry at all. The exact popup dimensions are deliberately **not**
pinned; only the properties §10 already states.

Six things Phase D inherits as decided rather than open:

- **The scaffold is deliberately unwired.** `lib.rs:310` still calls `toggle_selected`
  unconditionally and `main.rs` still reads argv itself. Wiring a panicking scaffold into
  the live key path would have made every `Space` in the PTY suite panic — protected
  tests, and a *drop* in the score, which §2's ratchet reads as progress. T5 and T7 do the
  wiring as part of their own task; **T7 must also rewire `main.rs` to call `parse_args`**,
  and `s12_mini_resolve.rs` pins the two behaviours that must survive that (`--version`
  and the first-positional state-path hook).
- **The argv contract** is written down as `parse_args`'s doc comment (not in `SPEC.md`,
  which §3 protects and §11 reserves for a human) and asserted case by case. The load-
  bearing rule: `--mini`'s operand is the next argument iff it does not start with `-`, so
  `petri --mini state.json` is a *pin*, not a state path.
- **`resolve_mini` walks ancestors against `projects.json`'s own `path` fields**, deepest
  first, rather than porting the scanner's `resolve_root` — `petri` depends on neither
  `swab` nor `gix`, and reading the scanner's stored answer back is strictly stronger
  against the "two answers to which project is this directory" invariant than a second
  implementation would be. It subsumes the git-toplevel walk for free.
- **An ambiguous `--mini <NAME>` resolves to the most recently active match** — greatest
  `last_activity_at`, `None` last, ties broken by `path` ascending. Names are not unique
  (the fleet has three `smoke`s, per `SelectionAnchor`'s doc comment), so this had to be
  decided; the first draft errored out instead, and the human's call was that erroring is
  technically safer and practically useless when the project already knows which one you
  mean. It is not a coin flip: it is `swab`'s own ordering (`scan.rs`'s sort), the same
  judgement the Dashboard's top-of-list encodes, so "the `smoke` you mean" is the `smoke`
  you were last working in. `resolve_mini` re-derives that order rather than reading
  `radar.projects`' order, so the answer does not silently follow a future change to the
  writer's sort. **Accepted cost:** a pane pinned by an ambiguous name can move to the
  other project once that one becomes the more recently active; pin by path to avoid it.
- **With the popup open, `Space` closes it and does nothing else** — even on a header,
  whose own binding would otherwise toggle the section. §7's table states both rows
  ("header → unchanged" and "popup open → close it") without saying which wins; this
  resolves it as popup-wins, one keypress one effect. The cost is a second `Space` to
  collapse a section while the popup happens to be open. The **second** decision in this
  phase worth a human veto; it is pinned by
  `space_on_a_header_while_the_popup_is_open_closes_it_without_toggling`.
- **The RUNNING header's count is `running_membership`, not a `status_bucket` filter**, so
  a cold parent pulled in by an active worktree child is counted. `normal.json` has no
  `parent_path` at all, which would have made that test vacuous, so it builds the pull-in
  case and guards it with an `assert_ne!` against the plain filter.

---

## 7. Phase D — the mounts (delegable)

### T5 — mount #30: the Dashboard popup · `[hard]`
- **Mutable:** `petri/src/dashboard.rs`, `petri/src/lib.rs`, `petri/src/focus.rs`
- **Do:** contextual `Space` per the proposal §7 table; `MECH-1` popup; the
  ≤80%-of-terminal rule that switches to the full-screen render. (The
  `FocusTarget::Section` empty state is **no longer part of this task** — T1b builds it, so
  T5 is purely the mount wiring: deciding the target from the cursor, and the popup's
  lifetime and geometry.)
- **Gotcha — this is the behaviour change:** `lib.rs:310` currently calls
  `dstate.toggle_selected(radar)` for `Space` unconditionally. It must branch on
  `DashRow::Header` vs `DashRow::Project`. `Enter` (`lib.rs:320`) already branches exactly
  this way — copy that shape rather than inventing one.
- **Gotcha:** `Enter` is unaffected. Do not "unify" the two keys.

### T6 — re-point the Browser's popup · `[easy]` · **DROPPED (2026-09-09, human call)**

> **The task's own premise is false, and the ratchet cannot grade it.** Kept here rather
> than deleted, because the reasoning is the record for why #32 ships narrower than its
> text reads.
>
> `s5_snapshot.rs::detail_popup_reaches_detail_fields_when_neither_placement_fits` asserts
> that `github_url` appears in the `Space` popup at **60×10** — that assertion *is* issue
> #35's reason for existing: the popup is the only way to reach detail-only fields when
> neither inline placement fits. The focus panel's `Repo` rung, which is where a URL would
> come from, gates at `(56, 28)`. A ten-row terminal cannot admit it at any popup size, so
> the swap makes that protected test fail by construction, not by a sizing mistake.
> Widening the gate instead breaks `s11_focus_plan.rs`.
>
> On top of that, **no test in Phase D grades T6** (BASE 52 = 23 in `s11_focus_mount.rs` +
> 29 in `s12_mini_resolve.rs`), so it can only ever *lower* the score, and §2's keep rule
> reverts every attempt — the same shape as T0b, which is why that one was pulled forward
> into Phase A as cloud work.
>
> **Decision:** drop it. #32 closes for the Dashboard popup and `--mini` only; the
> Browser's `Space` popup and its inline detail pane both keep the fact sheet, and the
> re-point becomes a follow-up issue. The rejected alternatives were (b) authorise both a
> spec change and an `s5_snapshot.rs` edit, accepting that the popup stops being #35's
> escape hatch, and (c) render `focus_lines` only when the inner rect clears the `Repo`
> gate and fall back to the fact sheet below it — two detail layouts in one screen
> swapping on a size threshold, the least predictable of the three.

- **Mutable:** `petri/src/browser.rs`
- **Do:** the `Space` popup renders `focus_lines` instead of the fact sheet. The binding,
  the popup geometry and the `Esc` behaviour are all unchanged.
- **Gotcha:** the *inline* detail pane (beside/below, issue #35) is **not** in scope and
  must keep working. Only the popup changes. Existing `s5_*` tests are the safety net and
  are protected — if one of them fails, the change is wrong; do not edit the test.

### T7 — `petri --mini` · `[hard]`
- **Mutable:** `petri/src/main.rs`, `petri/src/lib.rs`, `petri/src/focus.rs`
- **Do:** arg parsing, cwd resolution, alt-screen run loop reusing the existing poll loop.
- **Gotcha — the arg-parsing trap:** `main.rs` today treats **the first positional arg as a
  state-file path**, a documented test hook the PTY suite depends on, and deliberately has
  no `clap` (`SPEC.md` §10 doesn't list it). So `petri --mini foo` is ambiguous. Contract to
  implement: `--mini` takes its optional target as the argument **immediately following
  it**; the state-path hook remains the first argument that is not a flag and not `--mini`'s
  operand. Do not add `clap` to win this.
- **Gotcha:** re-resolve the project by path/name **every tick**. Never cache the index —
  the scanner re-sorts `radar.projects` on every scan and `SPEC.md` §4.3 records this as a
  live bug already found once.
- **Gotcha:** the missing-state-file check happens *before* entering the alternate screen
  (§4.4). The not-a-project message follows the same rule.

**Phase D exit:** `afk-verify.sh` prints 0; `make check` green.

### Actual result (2026-09-09)

Landed as **two** commits, not three — T6 is dropped (see above). `afk-verify.sh`: 52 → 29
(T5) → 0 (T7). `make check` exits 0 workspace-wide.

Six things worth carrying forward:

- **`focus_placement`'s switch is tied to `plan_rungs`' `Path` gate `(30, 10)` plus the
  border**, i.e. `FOCUS_POPUP_MIN_WIDTH/HEIGHT = 32/12`, not to a hand-fitted constant. An
  overlay earns its place only once its content rect can carry more than the three
  unconditional rungs the floor guarantees — and that rule reproduces §10's pressure table
  exactly (60×20 is a popup at 48×16; 48×14 misses by one row at 38×11). The popup is also
  capped at 64×24 so a large terminal gets an overlay rather than a bordered full screen;
  the 80% bound still applies above the cap.
- **`render_focus_overlay` is a separate call from `dashboard::render`**, not a branch
  inside it. That signature is pinned by `s6_snapshot.rs`/`s9_feed_render.rs` and carries
  no `Prefs`, which the `ACTIONS` rung needs; keeping it separate also keeps `MECH-1`'s
  ordering explicit at the call site. `render_current` grew a `prefs` parameter — it is
  private, so that was free.
- **`toggle_selected` is untouched.** `press_space` is a new entry point and only the key
  handler is rewired, because `Enter` on a header and `s6_dashboard.rs` both still want
  `toggle_selected`'s current shape. A PTY test does press `Space` (`s6_pty.rs:140`) but
  only asserts liveness, so the behaviour change is invisible to it.
- **The ambiguous-name tie-break is case-insensitive**, then byte-wise for determinism.
  §6 said "ties broken by `path` ascending" without saying under which ordering, and plain
  byte order sorts `repos/JKrag/lantern` ahead of `repos/aaa-first/lantern` — ASCII puts
  every capital before every lowercase letter, which is not what "alphabetically first"
  means to anyone reading a fleet list. `tied_matches_break_by_path_not_by_radar_order`
  fails under a raw compare. macOS paths are case-insensitive anyway.
- **`--mini`'s chrome is itself responsive** and appears only at 30×8 and above, so a
  header can never push the panel below its own 24×6 floor. That reproduces §3.2/§3.3/§3.4
  as written. The footer those mockups show is **not** chrome: it is the `Actions` rung in
  its degraded, label-less form, which `plan_rungs` already admits down to the floor. The
  header's right-hand group is left empty for `TQ-b`, not half-built.
- **A `--mini` target that stops resolving mid-run renders the error in-pane** rather than
  exiting. The preflight (before the alternate screen, §4.4) covers the case the user can
  act on; a pane pinned in a split for days must not vanish because one scan dropped a
  project.

Two things Phase E and the attended pass inherit:

- **`petri/src/focus.rs` gained an inline `mini_mount_tests` module** — five `TestBackend`
  tests over `render_mini`'s chrome arithmetic. It is the one part of the `--mini` mount
  that is neither `plan_rungs`' (pinned size-by-size in `s11_focus_plan.rs`) nor the run
  loop's, and it was otherwise ungraded. They live in `src/` rather than `tests/`, so
  `EXPECTED_BINARIES` is unchanged.
- **T5 briefly shipped a glyph-allowlist violation** — the popup's border was
  `BorderType::Rounded`, whose `╭╮╰╯` are not on the allowlist, exactly the trap
  `focus.rs`'s module doc had already called out. `glyph_portability.rs` did not catch it,
  because the gate does not see ratatui-generated border characters at all. That hole is
  `IDEAS.md` §5's, still open, and it is now known to be reachable rather than theoretical.

---

## 8. Phase E — near-independent work

Neither needs `focus.rs`. **But both mutate `dashboard.rs`, as does T0b** — so they are not
freely parallel with Phase B. Sequence them *after* T0b and they are conflict-free; run them
concurrently with it and you get a three-way conflict on the one file.

### TQ-a — quota in the header · `[easy]`
- **Depends on:** T0b (file-ordering only, not logically)
- **Mutable:** `petri/src/dashboard.rs` (`header_lines`)
- **Do:** `5h {n}% · 7d {n}%` in the header's right group, with the elision ladder
  `scan` → `projects` → clock → compress to `16%/1%` → drop.
- **Gotcha:** the sensor is already built and populating `Radar.quota` — this is display
  only. `petri` never calls `read_quota`; it reads the already-parsed `Radar.quota` off the
  state file like every other field. So there is no reason for this task to open
  `swab/src/sensors/quota.rs` at all, and `swab/` is protected (§3).
- **Gotcha:** **do not render `context_used_pct`.** It is owned by whichever session wrote
  `last-status.json` last and is not attributable to any project (`DATA-5`).
- **Gotcha:** `None` omits the segment. Never `0%`.

### TQ-b — the same header line in `--mini` · `[easy]`
- **Depends on:** T7 and TQ-a
- **Mutable:** `petri/src/focus.rs`
- **Do:** reuse TQ-a's segment builder in `--mini`'s header. Split out from TQ-a precisely
  because it is the only part that depends on `--mini` existing — leaving it inside TQ-a
  made a task drawn as independent silently depend on the last task in Phase D.

### T8 — the lush tier (#33) · `[medium]`
- **Depends on:** T0b (file-ordering only)
- **Mutable:** `petri/src/dashboard.rs`
- **Do:** a third density tier: roomy + rungs R4 and R7 as two extra card rows, plus the
  surplus-priority rule — cards take a **bounded** +2 rows first, the feed takes the
  remainder under its existing `events + 2` bound.
- **Gotcha:** the tier must be driven by the row budget, like `COMPACT_TIER_MAX_CONTENT_ROWS`
  — not by width (`SPEC.md` §3.2 is explicit, and the earlier width-driven assumption was
  already wrong once).
- **Gotcha:** the feed must still yield entirely when any section was skipped or truncated.
  That rule is unchanged and its test must keep passing.

### Dependency graph

```
                    ┌── TQ-a ──┐                        (dashboard.rs)
                    ├── T8 ────┤                        (dashboard.rs)
  A (incl. T0b) ────┤          │
                    │          │
                    └── T1a ──┬┴─ T1b ──┬── T2          (focus.rs)
                              │         ├── T3
                              │         └── T4
                              │              │
                              └──► C ──┬── T5 ─── T6
                                       └── T7 ─── TQ-b
                                                   │
                          ──► make check ──► PTY (§9) ──► done
```

Two rules this graph encodes, both learned by drawing it wrong first:

- **`dashboard.rs` has three claimants** (T0b, TQ-a, T8). T0b landed in Phase A, which
  leaves two — order-independent, but they must still not run *concurrently* with each other
  on the same tree.
- **`focus.rs` has five claimants** (T1a, T1b, T2, T3, T4). T2/T3/T4 are logically
  independent of one another but all edit the same file, so a genuine parallel fan-out needs
  them on separate branches with a merge step. Sequential is the simpler default; parallelism
  buys little here since each is a small task.

---

## 9. What must **not** go in an AFK queue

- **PTY tests.** `SPEC.md` §8 and `IDEAS.md` §5 both call this the flakiest layer in the
  repo, and an unattended loop cannot tell a flake from a defect — it either halts on a
  false failure or learns to ignore the layer. Add `s11_pty_focus.rs` and `s12_pty_mini.rs`
  **attended**, after Phase D, as the last step before opening a PR.
- **Any `SPEC.md` edit.** Three decisions in this plan change the spec — contextual `Space`,
  the surplus-priority rule, quota in the header. Those are yours to make; the delegate
  implements the decision, it does not record it.
- **Glyph allowlist additions.** If a task needs a new glyph, that's a signal to stop and
  look, not a line to add. (The `┌┐└┘` gap noted in `IDEAS.md` §5 is a separate pre-existing
  issue — fix it deliberately, not as a side effect of a round trying to go green.)

---

## 10. AFK-specific setup

If you do go the AFK route:

- **`cargo sweep` first.** `CLAUDE.md` records `target/` reaching 441k files / 22.7GiB, with
  a bloated cache measurably *slower* than a cold one. An overnight run of repeated
  `clippy --all-targets --all-features` compounds exactly this. Prune before starting.
- **Iterate scoped, gate wide.** The round loop uses `cargo test -p petri` (~9s); `make
  check` (~28s, workspace-wide) runs only at a phase exit. That's `CLAUDE.md`'s own rule and
  it's worth ~3x on every round.
- **Clean tree + `BASE=$(git rev-parse HEAD)` before starting**, per the skill's start-gate.
- **Confirm `make check` is green at `BASE` before starting — this is not ceremony.** The
  round loop scores `cargo test -p petri`, so a failure anywhere else in the workspace is
  invisible to the ratchet right up until a phase exit, where it surfaces as "the phase
  didn't pass" with no indication the cause predates the job. An unattended loop will then
  burn its remaining rounds trying to fix code it never touched.

  This is not hypothetical. On 2026-09-08 two tests in `swab/src/sensors/quota.rs` began
  failing with no code change: they pinned a literal `"2026-08-09T06:32:11Z"` against a live
  `Utc::now()`, and the 30-day `MAX_RESET_HORIZON_S` guard started dropping it exactly
  thirty days later. Fixed in `90aed29` by injecting the clock, but the general shape —
  **a time-bomb test in a crate this job does not touch** — is exactly what a baseline check
  catches and nothing else in the loop would.
- **Suggested budget per phase:** Phase B — 6 rounds, 90 min, retry cap 3. Phase D — 8
  rounds, 2h, retry cap 3 (T5/T7 are the two `hard` ones and where escalation is most
  likely). Phase E — 4 rounds, 45 min.
- **Escalate rather than guess** if `afk-verify.sh` prints `9999` twice in a row: two
  consecutive build failures on a scaffolded API usually means the scaffold's signature is
  wrong, which is a planning bug the delegate cannot fix by trying harder.

---

## 11. Definition of done for the whole job

1. `make check` exits 0 (workspace-wide, all four crates).
2. `make check-all` exits 0 — run before the PR, per `CLAUDE.md`.
3. ~~PTY coverage added attended (§9).~~ **Done (2026-09-09)** — `s11_pty_focus.rs` (6
   tests) and `s12_pty_mini.rs` (9). The `--mini` file is the one that mattered: nothing
   graded `run_mini` → `mini_poll_loop` → `render_mini_frame`, so Phase D could have
   scored a perfect 0 with no run loop at all. All fifteen were mutation-checked rather
   than merely observed green — blanking `render_mini` fails the two rendering tests and
   neither quit test, dropping `Esc` from the mini key match fails only the `Esc` test,
   and reverting `Space` to `toggle_selected` fails four focus tests while correctly
   leaving the header-toggle test passing. `pty_support/mod.rs` gained `spawn_with_args`
   (additive; both existing entry points delegate to it with an empty slice) because
   `Session` could otherwise only ever spawn `petri <state>`, and `--mini`'s whole
   contract is about how a flag and that positional interact. `EXPECTED_BINARIES` 34 → 36.
4. `SPEC.md` updated with the three decisions from §9, by a human.
5. `IDEAS.md`'s `SURF-8` gets its `DONE` pointer and the narrative moves to `IDEAS_LOG.md`
   — that's `IDEAS.md`'s own stated convention, and the deferred-rung list stays behind.
6. Issues closed, with one scope caveat worth stating rather than discovering on the issue:
   - **#30, #31** — fully.
   - **#32 — for the Dashboard popup and `--mini` only.** T6 was dropped (§7): re-pointing
     the Browser's `Space` popup at the focus panel would break `s5_snapshot.rs`'s
     detail-fields-at-60×10 assertion by construction, since the `Repo` rung gates at
     `(56, 28)`. Both the Browser's popup and its inline detail pane keep the fact sheet.
     Close #32 saying exactly that, and open the re-point as a follow-up — `ACT-7`'s text
     says "the detail pane," so the issue implies more than shipped unless it is scoped
     explicitly.
   - **#33** — if T8 landed.
   - **#29 — MVP only.** The dedicated-token-TUI hand-off and `DATA-5` (`ctx%`) stay open.
