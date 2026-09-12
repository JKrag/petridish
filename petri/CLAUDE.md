# petri

The terminal dashboard. See the root `CLAUDE.md` for the workspace-wide picture; this file
is the `petri/`-specific detail — PTY test authoring rules — that only needs to load when
you're working under `petri/tests/`.

## Testing: PTY harness

**PTY tests:** `petri/tests/pty_support/` drives the real binary through a pseudo-terminal.
Assert against a reconstructed screen grid, never the raw byte stream — a partially-painted
frame must show up as wrong content in a specific cell, not as a coincidentally-passing
substring match. `settle()` applies its quiet window only *after* the first byte arrives;
applying it to an empty buffer conflates "finished painting" with "not started yet" and
makes the suite fail whenever the machine is busy.

**Never settle-then-assert. Every wait states the condition it is waiting for.** A quiet
window is a guess about how long the binary takes; `screen_until` waits for the frame you
are about to make a claim about. This is not a style preference — it was the single root
cause of every flaky test in this suite, and the numbers were not small: measured at
eight-way concurrency, `s8_pty_repick` failed 17 runs in 24, `s8_pty_actions` 15 in 24,
`s8_pty_filter` 10 in 24. Three rules follow from fixing them, each learned by getting it
wrong first:

- **The predicate must be false for the pre-keystroke frame.** Waiting for `/beta` after
  Enter proves nothing — it is already on screen, so the wait returns instantly and the
  assertion runs against a frame Enter never touched. Wait for what the keystroke *changes*
  (there, the input's block cursor disappearing).
- **An absence needs `settle_until_gone`, not a bare wait.** "The popup is closed" is
  equally true of a frame that has not repainted yet.
- **A keystroke that shells out or hands over the terminal is not observable on the grid at
  all.** A hand-off restores the very screen it left, so no grid predicate can tell
  "finished" from "not started" — use `settle_until_raw` and watch for the second
  alternate-screen entry. Sending a key before that point does not just race: the byte is
  swallowed by the line discipline in canonical mode and never delivered, which surfaces
  ten seconds later as "the child did not exit".
- **Startup is the same trap, and `"petri"` is not the marker for it.** petri prints
  `prefs::load`'s "preferences file ... missing" warning to a normal stderr *before*
  `enable_raw_mode` — deliberately, so it cannot corrupt the first draw (`lib.rs`, "Step
  1.5"). So the stream is non-empty and the grid is non-blank while the terminal is still in
  canonical mode, where the first keystroke is swallowed exactly as above. Wait for a header
  BADGE (`pty_support`'s `DASHBOARD_HEADER` / `BROWSER_HEADER`), which cannot be painted
  until after the alt-screen entry that follows raw mode — or, when a raw wait is what you
  have, for `alt_screen_entries(...) >= 1` directly.
- **Name the screen, not a word that happens to be on it.** `"browser"` is in the
  *Dashboard's* footer (`Enter open/browser`), so waiting for it after `Tab` accepts the
  pre-`Tab` frame and hides a lost keystroke behind a passing wait. `" petri · browser "` is
  on one screen only. Before believing any needle, grep the other screen for it.

**`make flake-hunt` measures this rather than guessing at it** (`petri/scripts/flake-hunt.sh`;
runs each PTY binary N times at concurrency, reports a per-test failure rate). Concurrency is
the mechanism — these races are perturbed by many PTY sessions running at once, which is what
`cargo test` does, and *not* by CPU load: one test measured 3 failures in 48 at eight-way
concurrency and 0 in 25 under heavy CPU spinning. It is deliberately not part of `make check`
— it takes minutes, and a gate that slow gets skipped. Run it before a release, or when a PTY
test fails once and you need to know whether that meant anything.
