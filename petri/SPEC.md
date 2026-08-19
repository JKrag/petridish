# `petri` (Rust edition) — specification

**Status:** spec for work not yet started. Supersedes `ARCHITECTURE.md` §6, which
described the Python/curses build and is now a pointer here.

`petri` is the interactive frontend of petridish: a terminal dashboard over
`~/.petridish/projects.json`. This document is the authoritative spec for the Rust
build. Terminology is defined in `CONTEXT.md` — **Dashboard**, **Browser**,
**petripy**, **state file** and **preferences file** are used here in exactly the
senses defined there.

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
  unchanged. `Enter`/`Esc` closes it, `Esc` clears.
- **Scrolls**, with a scrollbar. "Every project must be reachable" is this screen's
  rule; truncating it would break that. Contrast the Dashboard (§3.2).
- Footer keymap advertising **only keys actually bound**.

### 3.2 Dashboard

The ambient monitor: "does anything need me?" across a fleet of unattended runs.

- Header: `petri · dashboard`, project count, clock, last scan duration.
- **`RUNNING`** — membership per ADR-0001: a project counts as running if its own
  bucket is `active` *or* it has an active worktree child. Ordered **quietest
  first** (longest-silent at the top — the stalled run is the one that needs you).
  Label degrades to `RECENT` when nothing in the section has an agent at all,
  because `RUNNING` would then overstate it.
  - Roomy cards: glyph, name, `silent 3m · claude-code`, branch, dirty marker,
    `commit 1d ago (you)`, `~`-abbreviated path, truncated session id.
  - **Worktree nesting:** an active worktree indents under its parent when the
    parent is also in this section; when it is not, it falls back to the
    `name (in parent-name)` suffix form `swab list` uses. Display-only — the
    `status_bucket` in the state file is never touched (ADR-0001).
- **`IN FLIGHT` / `STALE` / `COLD`** — compact rows: name, branch, `✎N`, commit
  age, `gh` marker. A parent with worktree children in any bucket shows
  `name · N worktrees` on its own row rather than listing them.
- **Sections are collapsible.** A collapsed section keeps its header and count and
  yields its rows' space to the sections left open. Defaults: `RUNNING` and
  `IN FLIGHT` expanded, `STALE` and `COLD` collapsed. This is how real estate gets
  allocated on this screen — deliberately by the user, not by a fixed priority
  ladder.
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
- **Staleness banner:** when the state file's `updated_at` is older than 24h
  (matching `swab doctor`'s freshness check), render normally *and* show a
  persistent banner. The screen must degrade visibly, never silently lie about
  freshness.

---

## 4. Application shell

- **Auto-poll:** `stat` the state file's mtime on a short timer (2–5s) and re-read +
  re-render only when it changed. A plain stat-poll, not a file watcher — no new
  dependency.
- **Missing state file:** checked *before* entering the alternate screen. Print the
  same message `swab list`/`swab path` use —
  `no state file at {path}; run 'swab scan' first` — and exit 1. Never a blank or
  broken screen.
- **Resize and tiny terminals** must not panic. Clip, or show a `resize terminal`
  message. This includes the degenerate 0×0 a freshly-forked pty reports.
- **Schema drift:** `#[serde(default)]` on every field that can be absent, so a
  state file written by an older `swab` still parses. If `schema_version` is
  *greater* than the version this build knows, render normally but show a banner in
  the same slot as the staleness banner. Never hard-fail on a readable file.
- **Terminal restoration** on every exit path, including panic — install a panic
  hook that leaves the alternate screen and disables raw mode before unwinding. A
  panic that leaves the user's terminal in raw mode is a v1 blocker, not a polish
  item.
- **Colour:** use ratatui's ANSI 16-colour names (`Color::Green`, `Color::Yellow`,
  …) rather than fixed RGB. The dashboard lives inside the user's themed terminal
  and should inherit it. No truecolor palette in v1.

---

## 5. Keybindings (v1)

| Key | Action |
|---|---|
| `Tab` | switch Dashboard ↔ Browser |
| `j` / `k` / `↑` / `↓` | move selection (both screens) |
| `Enter` | Dashboard: on a row, jump to Browser on this project; on a section header, toggle it. Browser: **unbound** (see below) |
| `/` | Browser: open type-ahead filter |
| `Esc` | close/clear the filter |
| `Space` | Dashboard: collapse/expand the current section |
| `q` | quit |

The footer must advertise only keys that are actually bound. No launch, open-editor
or resume-session actions in v1 — the Raycast extension already covers those, and
`petri` staying read-only is intentional.

**`Enter` is deliberately unbound in the Browser, and must not appear in its
footer.** The Dashboard's `Enter` exists to cure a real itch ("my fingers wanted to
arrow down to *something*"); the Browser is where you land, and in a read-only v1
there is nothing further to open. Advertising `Enter` there would recreate the same
dead-key feeling one screen down. When actions do arrive (open in editor, resume
session), the Browser's `Enter` is where they belong.

---

## 6. Preferences file

`~/.petridish/petri.toml`, owned and written by `petri` alone. Holds which Dashboard
sections are collapsed, and the last screen. `swab` never reads it; it is
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
- **Density auto-switch and the `z` override.** Superseded by collapsible sections,
  which solve the same real-estate problem better. Cards are always roomy in
  RUNNING, rows always compact elsewhere.
- **COLD as a `·`-joined one-line name list.** A collapsed COLD section plus the
  Browser covers it; the joined line is a curses-era space-saving trick.
- **Responsive detail pane** (beside vs below). Always right, hidden when too
  narrow.
- **Inline card expansion** on the Dashboard.
- **A non-interactive `petri dash`** printing one frame and exiting (pipeable into a
  tmux status pane). Free while screens were pure functions; now needs an explicit
  off-screen buffer render.

---

## 8. Verification

Four layers. Full reasoning: ADR-0003. This work is intended for unattended
(`delegate-afk`) execution, so the bar is "the machine can tell done from compiles".

1. **Pure-state unit tests.** Port the *test cases* — not the code — from
   `tests/test_tui_state.py` and `tests/test_screens.py`: grouping, bucket
   membership, worktree rollup, quietest-first ordering, silence seconds, humanised
   durations, selection movement across section boundaries, clamping at both ends,
   the empty selection, and re-filtering out the selected row.
2. **`TestBackend` buffer snapshots.** `assert_buffer_lines` over each whole screen
   at three geometries: `80×24`, `200×50`, and `40×10`. These are self-referential
   goldens, *not* comparisons against petripy.
3. **PTY end-to-end tests** against the real binary, following
   `tests/test_tui_pty.py`'s hard-won lessons: keep draining the pty while waiting
   or the child blocks in `write()` and looks like it ignored your keystroke; set
   the winsize explicitly or a forked pty starts at 0×0. Cover: it starts, `Tab`
   switches, `/` filters, `j`/`k` move, resize survives, `q` returns the shell and
   the terminal is not left in raw mode.
   **These must be deterministic by construction**, because this layer is
   empirically the flakiest one in the repo — two of the Python equivalents pass
   locally and failed on the macOS CI runner. Requirements:
   - **Assert against a settled full-screen snapshot, not the raw byte stream.**
     This is the big one. The captured failing CI frame was
     `petri · dashboard … ════ … ──── … tab browser z density q quit` — the whole
     RUNNING section simply absent, i.e. a partially-painted frame. Reading the pty
     byte stream also means "lines" are stream segments, not screen rows, which is
     what broke the other assertion. Read until the stream goes quiet, then assert
     on the reconstructed screen.
   - **Inject a fixed clock.** The `⚠` stalled-run glyph is derived from silence at
     *render* time on purpose (`glyph_for` deliberately bypasses the stored
     `agent.state` so the glyph can't disagree with the live silence counter next
     to it), and the Python fixture builds its offsets from `datetime.now()`. Any
     wall-clock-derived assertion is a latent flake.
   - Set the winsize explicitly (a forked pty starts at 0×0) and pin `LANG`/`LC_ALL`
     to a UTF-8 locale.

   A flaky layer is worse than no layer when nobody is watching: an unattended
   agent cannot tell a flake from a defect, so it either halts on a false failure
   or learns to ignore the layer.
4. **Human smoke test** — but as confirmation, not as the gate. "It works, and I
   have an idea for a change" is the expected shape of it.

### Fixtures

Four committed fixtures under `tests/fixtures/`, replacing the current
single-project `projects.golden.json`:

| File | Shape |
|---|---|
| `minimal.json` | one project |
| `normal.json` | ~15 projects, mixed buckets |
| `loaded.json` | ~70 projects, worktrees, every bucket populated |
| `hostile.json` | empty project list · all-cold · non-repo · null branch · 200-char name · CJK/emoji name · absent quota · `updated_at` 3 days old · worktree whose parent is absent · `schema_version` from the future |

`hostile.json` is where an unattended agent's bugs actually surface. Every snapshot
and pure-state test runs against it.

### Gate

`make check` grows `cargo test --workspace`, and `ci.yml` grows a Rust job. `swab`
has **never** been in CI — that gap predates this work and is closed as part of it.
An unattended agent needs one command that is the entire truth.

---

## 9. Build order

Each slice is independently verifiable and committable — one commit per verified
slice, matching `delegate-afk`'s keep/revert checkpoint boundary.

| # | Slice | Verified by |
|---|---|---|
| S0 | Rename the Python `petri` console script to `petripy`; repoint `install.sh`'s venv anchor from `petri` to `petridish-installer`; deprecation note in `README.md` | `pytest`, `install.sh` runs |
| S1 | Workspace refactor; extract `petridish-core` (schema out of `swab`); `swab` behaviour unchanged. `make check` + `ci.yml` gain `cargo test --workspace`. Repoint the doc references to `swab/src/schema.rs` (in `CLAUDE.md`, `CONTEXT.md`, `src/petridish/schema.py`) at their new home | existing `swab` tests green, unmodified |
| S2 | Fixture corpus (§8) | consumed by S3+ |
| S3 | `present` helpers into core; `cli.rs` refactored to call them | existing `cli.rs` tests green, unmodified |
| S4 | **`petri` walking skeleton**: real terminal, reads state file, header + flat list, `q` quits with the terminal restored, mtime poll, missing-file → exit 1, resize-safe, panic hook | first `TestBackend` snapshots + first PTY test |
| S5 | Browser: grouped list, selection across sections, detail pane, `/` filter, scrollbar, footer | snapshots ×3 geometries + PTY keystrokes |
| S6 | Dashboard: sections, roomy cards, quietest-first, worktree nesting, collapsible sections, cursor + `Enter`→Browser, staleness banner | snapshots + PTY |
| S7 | `Tab` switching + `petri.toml` persistence | snapshots + PTY + corrupt-toml test |

**S0 comes first** so the Rust binary can take the `petri` name on `PATH` without
colliding, and petripy stays runnable as a fallback throughout.

**S4 is deliberately trivial in content** — it is where all the infrastructure risk
lives (terminal setup, event loop, poll timer, panic hook, snapshot harness, PTY
harness), and that is best proven against a screen too simple to be wrong.

### petripy's lifecycle

Frozen on arrival of the Rust build: no new features, but it stays installed and its
tests stay in `make check`, so it cannot rot into a broken fallback while still
being relied on. Delete `src/petridish/{tui,tui_state,screens}.py` and their tests
after a few weeks of real use on the Rust build — the same pattern the Python
scanner's deletion followed.

---

## 10. Dependencies

`petri` may take dependencies freely; the stdlib-only rule was always
`src/petridish/`-specific and never applied to Rust. Pin in `petri/Cargo.toml`:

- `ratatui` 0.30 (current release; `TestBackend::assert_buffer_lines` is what layer
  2 is built on)
- `crossterm` (matching ratatui 0.30's backend)
- `toml` for the preferences file (already a `swab` dependency)
- `chrono`, `serde` via `petridish-core`

Note for whoever wires this up: ratatui 0.30 split into `ratatui-core` /
`ratatui-widgets` / backend crates. Depend on the `ratatui` facade unless there is a
specific reason not to.
