//! `ACT-10` (petri/IDEAS.md §2), layer 2: the `/` filter query must be
//! visible on screen.
//!
//! The bug this gates: `BrowserState.filter_query` was stored and applied,
//! but never drawn. The only evidence a filter was active was that the list
//! got shorter — indistinguishable from a fleet that had gone quiet. These
//! tests render off-screen and assert the header chip, because that is the
//! one surface that has to differ between "filtered" and "not filtered".
//!
//! Two states are asserted separately and deliberately: typing (input open)
//! and applied-but-closed (`Enter` pressed, query kept). The second is the
//! one the original complaint was about — the first is at least implied by
//! the keystrokes you just made.

use petridish_core::schema::Radar;
use petri::browser::BrowserState;
use ratatui::{Terminal, backend::TestBackend};
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn load(name: &str) -> Radar {
    let text = std::fs::read_to_string(fixture_path(name))
        .unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("fixture {name} failed to deserialize into Radar: {e}"))
}

fn rendered_lines(radar: &Radar, state: &BrowserState, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal must construct");
    terminal
        .draw(|frame| petri::browser::render(frame, radar, state))
        .expect("draw must not error");
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect::<String>()
        })
        .collect()
}

/// The unfiltered visible-row count for `normal.json`, derived the same way
/// the chip's denominator is: whatever `BrowserState::new` makes visible.
fn unfiltered_total(radar: &Radar) -> usize {
    BrowserState::new(radar).visible.len()
}

#[test]
fn no_chip_when_no_filter_is_active() {
    let radar = load("normal.json");
    let state = BrowserState::new(&radar);
    let header = rendered_lines(&radar, &state, 80, 24)[0].clone();
    assert!(
        !header.contains(" of "),
        "an unfiltered Browser must not show a match count, got: {header:?}"
    );
    assert!(
        !header.contains('/'),
        "an unfiltered Browser must not show a query chip, got: {header:?}"
    );
}

#[test]
fn the_query_is_visible_while_typing() {
    let radar = load("normal.json");
    let mut state = BrowserState::new(&radar);
    state.filter_input = true;
    state.apply_filter(&radar, "alpha");

    let header = rendered_lines(&radar, &state, 80, 24)[0].clone();
    assert!(
        header.contains("/alpha"),
        "the live query must be drawn while typing, got: {header:?}"
    );
    assert!(
        header.contains('\u{2588}'),
        "input mode must show a cursor after the query, got: {header:?}"
    );
}

#[test]
fn the_query_stays_visible_after_enter_closes_the_input() {
    // The actual ACT-10 complaint: filter, press Enter, look away, look back.
    // The list is short and there is nothing on screen saying why.
    let radar = load("normal.json");
    let mut state = BrowserState::new(&radar);
    state.apply_filter(&radar, "alpha");
    state.filter_input = false;

    let header = rendered_lines(&radar, &state, 80, 24)[0].clone();
    assert!(
        header.contains("/alpha"),
        "a kept query must still be drawn once the input closes, got: {header:?}"
    );
    assert!(
        !header.contains('\u{2588}'),
        "a closed input must NOT show a cursor — that is what distinguishes it \
         from typing, got: {header:?}"
    );
}

#[test]
fn the_chip_counts_matches_against_the_unfiltered_total() {
    let radar = load("normal.json");
    let total = unfiltered_total(&radar);

    let mut state = BrowserState::new(&radar);
    state.apply_filter(&radar, "alpha");
    let matched = state.visible.len();
    assert!(
        matched > 0 && matched < total,
        "fixture assumption: \"ala\" must match some but not all of {total} projects, matched {matched}"
    );

    let header = rendered_lines(&radar, &state, 80, 24)[0].clone();
    assert!(
        header.contains(&format!("{matched} of {total}")),
        "the chip must read \"<matched> of <total>\", got: {header:?}"
    );
}

#[test]
fn a_query_that_matches_nothing_still_says_so() {
    // The worst case for the original bug: an empty list that looks exactly
    // like an empty radar. "0 of N" is the whole point.
    let radar = load("normal.json");
    let total = unfiltered_total(&radar);

    let mut state = BrowserState::new(&radar);
    state.apply_filter(&radar, "zzzz-no-such-project");
    assert_eq!(state.visible.len(), 0, "fixture assumption: this query matches nothing");

    let header = rendered_lines(&radar, &state, 80, 24)[0].clone();
    assert!(
        header.contains(&format!("0 of {total}")),
        "an empty filtered list must be distinguishable from an empty radar, got: {header:?}"
    );
}

#[test]
fn the_chip_does_not_displace_the_footer_keymap() {
    // SPEC.md §3.1/§5: the footer advertises only bound keys — and it must go
    // on doing so while a filter is up. This is why the chip lives in the
    // header: replacing the footer per-mode would either drop bindings from
    // the advertisement or claim ones the filter mode does not honour.
    let radar = load("normal.json");
    let mut state = BrowserState::new(&radar);
    state.filter_input = true;
    state.apply_filter(&radar, "alpha");

    let lines = rendered_lines(&radar, &state, 120, 24);
    let footer = lines.last().expect("there is always a last row").clone();
    for advertised in ["Tab Dashboard", "/ filter", "q quit"] {
        assert!(
            footer.contains(advertised),
            "footer must still advertise {advertised:?} while filtering, got: {footer:?}"
        );
    }
}

#[test]
fn the_count_survives_a_narrow_terminal_and_the_query_is_truncated() {
    // 40×10 is one of SPEC.md §9's named geometries, and the chip is the
    // widest thing ever appended to the header — which cannot wrap
    // (`Constraint::Length(1)`), so an over-long query would otherwise push
    // the count off the right edge. The count is precisely the part that
    // must not be lost: "0 of 15" vs. an empty radar is what ACT-10 is for.
    let radar = load("normal.json");
    let total = unfiltered_total(&radar);
    let mut state = BrowserState::new(&radar);
    state.filter_input = true;
    state.apply_filter(&radar, "alpha-project-with-a-very-long-query");

    let lines = rendered_lines(&radar, &state, 40, 10);
    assert_eq!(lines.len(), 10, "render must still produce a full frame at 40x10");
    let header = lines[0].clone();
    assert!(
        header.contains(&format!("0 of {total}")),
        "the match count must survive a narrow header, got: {header:?}"
    );
    assert!(
        header.contains('\u{2026}'),
        "an over-long query must be visibly elided, not silently clipped, got: {header:?}"
    );
    assert!(
        header.chars().count() <= 40,
        "the header must not exceed the terminal width, got: {header:?}"
    );
}

#[test]
fn truncation_keeps_the_end_you_are_typing_and_the_start_you_are_not() {
    // While the input is open the cursor is at the end, so the tail is the
    // live part — truncating it would make further keystrokes invisible.
    // Once closed, the head is what identifies the query.
    let radar = load("normal.json");
    let mut state = BrowserState::new(&radar);
    state.filter_input = true;
    state.apply_filter(&radar, "abcdefghijklmnopqrstuvwxyz");
    let typing = rendered_lines(&radar, &state, 40, 10)[0].clone();
    assert!(
        typing.contains("xyz"),
        "while typing, the tail of the query must stay visible, got: {typing:?}"
    );

    state.filter_input = false;
    let closed = rendered_lines(&radar, &state, 40, 10)[0].clone();
    assert!(
        closed.contains("abc"),
        "once closed, the head of the query must stay visible, got: {closed:?}"
    );
}


