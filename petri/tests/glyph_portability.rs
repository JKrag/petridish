//! Rust half of petri/SPEC.md §4.2's glyph gate — mirrors the *test cases*
//! from `tests/test_glyph_portability.py` (petripy/ncurses side), not the
//! `wcwidth`-specific assumption behind them. See that file (and
//! `src/petridish/CLAUDE.md`'s "The `wcwidth` incident") for the founding
//! story: `⚠` (U+26A0, Unicode 4.0) rendered as a **blank cell** on the
//! macOS 14 CI runner because ncurses asks libc's `wcwidth()` before placing
//! a character and macOS's tables lag the standard.
//!
//! `petri` never calls `wcwidth` — ratatui and crossterm compute cell widths
//! via the `unicode-width` crate, an independent, UCD-data-derived table with
//! its own maintainers and its own (much smaller) gaps. So this test checks
//! the thing petri actually depends on, `unicode_width::UnicodeWidthChar`,
//! rather than re-imposing the old "must predate Unicode 1.1 / macOS's
//! wcwidth tables" bar out of caution: that bar has no bearing on this
//! rendering path and would block plenty of glyphs (Braille patterns,
//! U+2800-28FF, Unicode 3.0, are `width() == Some(1)` and render fine — btop
//! uses them for exactly this kind of sparkline) that are not actually a
//! problem here. Port the *gate* (a deliberate, reasoned allowlist beats
//! trusting every future glyph to be fine by convention), not the
//! *assumption* (that "old" is the thing that makes a glyph safe).
//!
//! **What this checks:** every candidate must have `UnicodeWidthChar::width()
//! == Some(1)` — a single, unambiguous narrow cell, matching what ratatui's
//! default (non-CJK) cell math actually computes. A `None` (a combining mark
//! or variation selector with no width of its own — confirmed empirically:
//! U+FE0F and U+200D both come back `Some(0)`, not "the width of the base
//! character", so a glyph built from base+selector cannot be reasoned about
//! by checking the base alone) or `Some(2)` (a genuinely wide, CJK-derived
//! character) is a straightforward reject: either would misalign every
//! column after it.
//!
//! **What this deliberately does NOT check**, and why: East Asian Ambiguous
//! characters (`width_cjk() == Some(2)` while `width() == Some(1)`) render
//! double-width on a terminal explicitly configured for CJK ambiguous-wide
//! handling. Several glyphs already shipping in this UI — `●` U+25CF, `█`
//! U+2588 — are exactly this case (checked empirically; both come back
//! `width_cjk() == Some(2)`). Gating on the stricter CJK interpretation would
//! reject glyphs already in real, working use for a hazard that only
//! materializes under a specific terminal configuration `petri` isn't
//! targeting today. This is a visible, deliberate scope line, not an
//! oversight: if `petri` ever targets CJK-locale terminals as a first-class
//! case, this test needs a second, explicit CJK-mode pass — it should not
//! silently start failing on glyphs nobody re-evaluated.
//!
//! Scope: the three modules that actually build `ratatui::text::Line`/`Span`
//! content petri draws to the screen (`app.rs`, `browser.rs`,
//! `dashboard.rs`, `feed.rs`, `picker.rs`) — `lib.rs`/`prefs.rs`/`main.rs` only ever write to
//! stderr/disk, never through ratatui, so a non-ASCII char there (e.g.
//! `lib.rs`'s `eprintln!("...Enter→Browser...")` diagnostics) never goes
//! through cell-width computation at all and is out of scope, matching the
//! Python test's own `CURSES_MODULES` scoping principle: gate what's
//! actually drawn, not everything in the crate.
//!
//! Within each scoped file, only *production* code is scanned: `//` line
//! comments (covers `///`/`//!` doc comments too) are stripped per line, and
//! everything from the first `#[cfg(test)]` onward is dropped, since test
//! assertion messages (e.g. dashboard.rs's `"200 cols fits 3× ..."`) never
//! reach a real user's screen either. Known limitation: a `//` inside a
//! string literal would truncate that line early; none of the scoped files
//! currently have one (a fixture project name with `//` in it would be the
//! first to trip this), so the simplification is safe today but not
//! foolproof forever.

use std::fs;
use std::path::Path;
use unicode_width::UnicodeWidthChar;

/// Modules whose output ratatui actually draws to the screen.
const RENDER_MODULES: &[&str] = &[
    "app.rs",
    "browser.rs",
    "dashboard.rs",
    // Added after review caught the gap: both build on-screen content and both
    // held a glyph nobody had width-checked (`\u{2192}` in a feed row, `\u{258C}` as the
    // picker's cursor). The list has to track "what draws", not "what drew when
    // this gate was written" — a module added later is exactly the one whose
    // glyphs nobody reviewed.
    "feed.rs",
    "picker.rs",
];

/// Every non-ASCII character permitted in the modules above, with a reason.
/// Verified below (`allowed_entries_are_single_narrow_cells`) against
/// `unicode_width::UnicodeWidthChar::width`, not just asserted here.
const ALLOWED: &[(char, &str)] = &[
    ('\u{00B7}', "separator in header/footer lines"),
    (
        '\u{00D7}',
        "multiplication sign, e.g. \"×10\" fast-jump hint",
    ),
    ('\u{2014}', "em dash, prose only"),
    ('\u{2026}', "truncation marker (\"… +N more\")"),
    (
        '\u{2192}',
        "bucket-transition arrow in a feed row (\"active → stale\")",
    ),
    ('\u{2500}', "light section rule"),
    ('\u{2502}', "browser pane divider / grid column gutter"),
    ('\u{2550}', "heavy header rule"),
    ('\u{2581}', "sparkline level 1/8"),
    ('\u{2582}', "sparkline level 2/8"),
    ('\u{2583}', "sparkline level 3/8"),
    ('\u{2584}', "sparkline level 4/8"),
    ('\u{2585}', "sparkline level 5/8"),
    ('\u{2586}', "sparkline level 6/8"),
    ('\u{2587}', "sparkline level 7/8"),
    ('\u{2588}', "sparkline level 8/8, quota bar filled"),
    (
        '\u{25B2}',
        "agent glyph: stalled / staleness banner. Replaced ⚠ U+26A0 (Unicode \
         4.0), which rendered as a blank cell on the macOS 14 CI runner under \
         petripy/ncurses — see this file's module doc comment.",
    ),
    (
        '\u{258C}',
        "block cursor after the picker's custom-path input",
    ),
    ('\u{25BC}', "scrollbar end symbol"),
    ('\u{25CB}', "agent glyph: idle or finished"),
    ('\u{25CF}', "agent glyph: working"),
    ('\u{270E}', "uncommitted-files marker"),
];

fn production_code(path: &Path) -> String {
    let text = fs::read_to_string(path).expect("render module must be readable");
    let text = match text.find("#[cfg(test)]") {
        Some(idx) => &text[..idx],
        None => &text[..],
    };
    text.lines()
        .map(|line| match line.find("//") {
            Some(idx) => &line[..idx],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn render_modules_use_only_allowlisted_characters() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders: Vec<String> = Vec::new();

    for name in RENDER_MODULES {
        let path = src_dir.join(name);
        let code = production_code(&path);
        for (lineno, line) in code.lines().enumerate() {
            for ch in line.chars() {
                if !ch.is_ascii() && !ALLOWED.iter().any(|(c, _)| *c == ch) {
                    offenders.push(format!("{name}:{} U+{:04X} {ch:?}", lineno + 1, ch as u32));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "Character(s) outside the glyph allowlist in a ratatui-rendered \
         module (petri/SPEC.md §4.2):\n  {}\n\nIf the character is a single \
         narrow cell per `unicode_width::UnicodeWidthChar::width` (test \
         `allowed_entries_are_single_narrow_cells` in this file will confirm \
         it), add it to ALLOWED with a reason. Otherwise pick an \
         already-allowed equivalent.",
        offenders.join("\n  ")
    );
}

#[test]
fn allowed_entries_are_single_narrow_cells() {
    for (ch, why) in ALLOWED {
        assert_eq!(
            ch.width(),
            Some(1),
            "{ch:?} ({why}) must be a single narrow cell per unicode-width \
             — ratatui/crossterm's actual width source — got {:?}. A None \
             (needs a base character/selector) or Some(2) (wide) would \
             misalign every column after it.",
            ch.width()
        );
    }
}
