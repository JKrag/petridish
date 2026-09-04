# Handoff: petridish — menu-bar / native-dashboard brainstorm

**Repo:** `/Users/jankrag/repos/JKrag/project-radar` (petridish)
**Branch:** `master`, clean at handoff time
**Scope of this doc:** only the tail-end brainstorm about a menu-bar / richer
dashboard client. Earlier work (petri TUI, Raycast `list-projects` +
`search-project`) is done, committed, and not repeated here — see refs below.

## State of the world at handoff

- `petri` (stdlib curses TUI): built, working, user-tested. Auto-refresh and
  colored agent-state bulb fixed and committed (`6f553cd`).
- Raycast extension (`raycast/`): `list-projects` (view) and `search-project`
  (no-view) commands built, tested, user-confirmed working. MIT-licensed
  (core `petridish` stays GPL — user confirmed this split is legally fine,
  separate non-linked processes). Latest commit `fda094a` (Prettier
  formatting fix, output of a first `/exploratory-validation` run via the
  local model — see ledger/LEARNINGS.md in `delegate-to-local` skill for the
  ROI writeup, including one caught instruction violation: the local model
  ran `ray lint --fix` after being told not to edit files; the diff was
  verified pure-formatting before being kept).
- Open, unresolved: `ray lint` fails on `package.json`'s `"author": "jankrag"`
  (404s against Raycast's user-lookup API). User isn't ready to create a
  Raycast account/publish yet — parked, no action needed until they are.

## The brainstorm itself (no code written, nothing built)

1. Sketched a **Raycast menu-bar command** (`mode: "menu-bar"`,
   `MenuBarExtra` API) as a mockup artifact — user hadn't realized Raycast's
   "menu bar" feature means the real macOS menu bar, not a Raycast-internal
   popup. Mockup published at:
   `https://claude.ai/code/artifact/af77d83c-9e05-4bb6-979a-896e0e1aa5ab`
   (private artifact; user has not asked to build this yet).

2. User raised a real architectural objection: `MenuBarExtra` only renders
   *inside* the Raycast app — it's not a standalone process. Anyone who
   doesn't already run Raycast (Albert, Alfred users, or no launcher) would
   need to install Raycast just for a menu-bar icon. Agreed this makes
   Raycast an odd dependency for a menu-bar-only user.

3. Proposed **xbar/SwiftBar plugin** as the better-fit alternative: those
   host apps do nothing but run a script on an interval and render stdout as
   a menu-bar dropdown via a simple text DSL — matches petridish's existing
   "small stdlib script reads `projects.json`" model closely, no new
   toolchain, no npm project. Tradeoff: still a separate install (xbar or
   SwiftBar), and click-to-act is cruder than Raycast's action panel.

4. User then asked to zoom out further: what if the menu-bar tool (or a
   richer dashboard) were built **natively** instead. Options laid out,
   roughly by effort:
   - **Native Swift menu-bar app** (`NSStatusItem` / SwiftUI
     `MenuBarExtra`, standalone `.app`) — most native/polished, could grow
     into a real window later, not just a dropdown. Cost: brand-new
     toolchain (Swift/Xcode) for this project; code-signing/notarization
     becomes a real ongoing cost the moment this is shared with anyone else
     (unsigned local builds are fine for personal use).
   - **Python + `rumps`/PyObjC** (or PyQt/Toga for something richer than a
     menu bar) — stays in petridish's existing language, can import
     `petridish` directly as a library. Cost: breaks the stdlib-only rule
     for this component (already an accepted tradeoff for GUI clients, per
     the Raycast precedent in `IMPLEMENTATION_PLAN.md` §7 D6); PyObjC/Qt
     apps are heavier and slower to start than a native binary.
   - **Electron/Tauri** — explicitly ruled out as the wrong kind of weight
     for a glance-at-your-projects tool, and the most foreign toolchain of
     any option discussed.
   - My stated lean (not a decision): Swift is the best long-term answer if
     the user is open to learning/using it; `rumps` is the pragmatic
     middle ground if they'd rather stay all-Python across the ecosystem.
   - Agreed framing: whichever is chosen, it's a **fourth independent
     "project"** in the petridish ecosystem (same relationship `raycast/`
     already has to the core) — the zero-dependency core (`src/petridish/`)
     is untouched regardless of which GUI client gets built, since all of
     these only ever read `~/.petridish/projects.json`.

**No decision was made.** This was explicitly "thinking aloud" — the user
had not chosen a direction when the session ended.

## Suggested skills for the next session

- **`/grill-with-docs`** — the natural next step once the user is ready to
  commit to one of the four routes (Raycast menu-bar / xbar-SwiftBar /
  native Swift / Python rumps-Qt). Use it to pressure-test the choice
  against `IMPLEMENTATION_PLAN.md` and the existing `raycast/` precedent
  before writing any code, the same way the original TUI-vs-Raycast
  decision was made.
- **`/to-prd`** or **`/to-issues`** — once a direction is picked, to turn
  "which of these four" into a concrete scoped task list, mirroring how
  M11/M12 (petri) and R1 (Raycast) were specified before delegation.
- **`/delegate-afk`** — for the actual unattended build once a spec exists,
  following the same split-architecture pattern used twice already (pure
  logic module separated from thin native/UI glue) if the chosen route has
  an equivalent split available.
- **`/exploratory-validation`** — already used once successfully this
  session (see `delegate-to-local`'s `LEARNINGS.md`, entry under
  "First `exploratory-validation` hunt") for a CLI build/lint/test smoke
  test. Reuse the same pattern for whichever new client gets built, once
  it has memory/lint/type-safety-style checks the local model can run
  read-only.

## Things NOT to re-derive

- The zero-dependency rule for `src/petridish/` core and why GUI clients
  are exempt: see `IMPLEMENTATION_PLAN.md` §7 (ADR D6) and §9 (Raycast
  documentation section).
- The full Raycast toolchain gotchas (`.ts` import extensions, split
  type/value imports, `@types/react` pinning): documented in
  `IMPLEMENTATION_PLAN.md` §9 and `raycast/README.md` — don't rediscover
  them if a future native client somehow touches the Raycast code.
- GPL-core / MIT-Raycast licensing rationale: already resolved, user
  confirmed via their own research; would apply the same way to any new
  client (separate process, no linking).
