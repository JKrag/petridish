//! `petri`'s shared truecolor palette (petri/SPEC.md §4.1).
//!
//! One palette for both screens, not two that happen to coexist by accident.
//! `ACCENT` is the app's own identity color (badge, selection, dividers);
//! `FRESH`/`AGING`/`COLD` are the green→amber→grey silence gradient applied
//! wherever a color needs to preview how stale something is (a project row,
//! an agent glyph, a section label) — one gradient, reused, rather than a
//! flat color that says nothing about what it's labeling.
//!
//! History, for whoever next touches a `Color::` literal in `browser.rs` or
//! `dashboard.rs`: petri/SPEC.md originally mandated ratatui's ANSI-16 names
//! ("inherit the user's themed terminal") — a call made while this spec was
//! still the unstarted plan for a curses port, back when `petri` itself was
//! Python. That constraint was superseded by an explicit product decision,
//! made once ratatui was actually in use, to go truecolor for a distinct,
//! screenshot-worthy identity. `dashboard.rs` adopted it first (see git log:
//! "unify chrome onto the truecolor palette from the mockup"); `browser.rs`
//! lagged behind on ANSI-16 names until both screens were pulled onto this
//! shared module. Do not reintroduce a second, ad hoc palette in a new
//! screen — extend this one.
//!
//! This is a **product/aesthetic** decision, unrelated to the glyph
//! portability constraint (also in petri/SPEC.md §4) — that one exists
//! because of a real macOS `wcwidth` bug and is not relaxed by anything here.

use ratatui::style::Color;

/// The app's own identity color: badges, selection borders/highlights,
/// heavy chrome rules.
pub const ACCENT: Color = Color::Rgb(0x33, 0xe2, 0xac);
/// Silence gradient, freshest: an agent/project active within the last
/// `AGENT_WORKING_MAX_S` (still likely mid-turn).
pub const FRESH: Color = Color::Rgb(0x4f, 0xe6, 0xa0);
/// Silence gradient, middle: quiet long enough to be worth a glance.
pub const AGING: Color = Color::Rgb(0xf0, 0xb8, 0x4f);
/// Silence gradient, coldest: quiet long enough to actually worry about
/// (or, for a section label, "nothing in here is running").
pub const COLD: Color = Color::Rgb(0x6b, 0x7a, 0x74);
/// Dimmer than `COLD` — reserved for structural chrome (gutters between grid
/// columns, the COLD section's own label) that should recede further than a
/// merely-cold data point.
pub const DIMMER: Color = Color::Rgb(0x48, 0x54, 0x4f);
/// Primary foreground text (project names, headline fields).
pub const FG: Color = Color::Rgb(0xd9, 0xe6, 0xe0);
/// Secondary/meta text (counts, timestamps, session ids) — dimmer than `FG`
/// but not as receded as `DIMMER`.
pub const DIM: Color = Color::Rgb(0x64, 0x76, 0x6f);
/// Branch names specifically — distinct from `FG` so a row's branch field
/// doesn't compete visually with its name field.
pub const BRANCH: Color = Color::Rgb(0x8f, 0xa3, 0x9b);
/// Danger/dirty-state signal (uncommitted changes, errors).
pub const DANGER: Color = Color::Rgb(0xef, 0x6a, 0x5b);

/// The silence gradient by `AgentActivity` tier, using the canonical
/// Working/Recent/Idle thresholds (`petridish_core::schema::agent_state_for_silence`)
/// rather than a second set of cutoffs invented for color alone.
pub fn tier_color(activity: petridish_core::schema::AgentActivity) -> Color {
    use petridish_core::schema::AgentActivity;
    match activity {
        AgentActivity::Working => FRESH,
        AgentActivity::Recent => AGING,
        AgentActivity::Idle => COLD,
    }
}

/// The same gradient, keyed by `StatusBucket`, for section labels: RUNNING
/// reads fresh-green, IN FLIGHT amber, STALE/COLD grey — a section's label
/// color previews the state of what's inside it instead of every header
/// being one flat color regardless of contents.
///
/// STALE and COLD share `COLD` rather than COLD getting its own darker step
/// down to `DIMMER`: `DIMMER` is reserved for structural chrome that should
/// recede further than any *data* a user might need to read (see its own doc
/// comment), and a section header is exactly that — a label someone reads to
/// decide whether to expand the section. A bold `DIMMER` header measures
/// ~2.1:1 contrast against a typical dark terminal background, under WCAG's
/// 3:1 floor even for bold/large text; `COLD` measures ~4.4:1. The section's
/// own label text ("STALE" vs "COLD") still carries the distinction a fourth
/// color step would otherwise have made.
pub fn bucket_color(bucket: petridish_core::schema::StatusBucket) -> Color {
    use petridish_core::schema::StatusBucket;
    match bucket {
        StatusBucket::Active => FRESH,
        StatusBucket::InFlight => AGING,
        StatusBucket::Stale | StatusBucket::Cold => COLD,
    }
}
