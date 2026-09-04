//! Terminal-column arithmetic for text that has to fit a width budget.
//!
//! Every render path in `petri` is handed a budget in **columns** by ratatui's layout,
//! and several of them used to spend that budget in **characters** — `.chars().take(n)`,
//! `.chars().count()`. Those agree only while the text is ASCII. A CJK ideograph is one
//! character and two columns, so a project, branch, session id or filter query containing
//! one overruns its cell, and the overrun lands on whatever was drawn next: the match
//! count a filter chip explicitly reserves room for, or the column alignment of every row
//! below. `tests/fixtures/hostile.json` carries a CJK name precisely because this case is
//! supposed to be handled.
//!
//! These helpers measure with `unicode-width`, the same table ratatui and crossterm use
//! for their own cell maths, so text is measured the way the renderer will place it.
//!
//! Scope limit, matching `SPEC.md` §4.2's: this is `width()`, not `width_cjk()`. East
//! Asian *Ambiguous* characters count as one column, which is right everywhere except a
//! terminal explicitly configured for CJK-ambiguous-wide. Unambiguously wide characters —
//! the ones that actually break these budgets — are two columns under both tables.

use unicode_width::UnicodeWidthChar;

/// Display width of `s` in terminal columns.
///
/// Characters with no width of their own (combining marks, zero-width joiners) count as
/// zero rather than being rejected: they genuinely occupy no cell, and a caller budgeting
/// space should not be told otherwise.
pub fn width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// The longest **prefix** of `s` that fits `budget` columns.
///
/// Stops before a character that would straddle the budget rather than including it, so
/// the result is always `<= budget` — never `budget + 1` because the last character
/// happened to be wide. That off-by-one-cell overrun is the whole bug this module exists
/// to prevent, and it is invisible in ASCII testing.
pub fn take_width(s: &str, budget: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(c);
        used += w;
    }
    out
}

/// The longest **suffix** of `s` that fits `budget` columns. The mirror of [`take_width`],
/// for the one caller that keeps the end of a string rather than its start (the open
/// filter input, where the cursor sits after the last character typed).
pub fn take_width_end(s: &str, budget: usize) -> String {
    let mut kept: Vec<char> = Vec::new();
    let mut used = 0usize;
    for c in s.chars().rev() {
        let w = c.width().unwrap_or(0);
        if used + w > budget {
            break;
        }
        kept.push(c);
        used += w;
    }
    kept.iter().rev().collect()
}

/// `s` fitted to exactly `w` columns: padded with spaces when short, elided with `…` when
/// long. The ellipsis is itself one column, so a truncated result spends `w - 1` columns
/// on content and one on the marker.
pub fn fit_exact(s: &str, w: usize) -> String {
    let actual = width(s);
    if actual <= w {
        return format!("{s}{}", " ".repeat(w - actual));
    }
    if w == 0 {
        return String::new();
    }
    let head = take_width(s, w - 1);
    // Re-pad: dropping a wide character can leave the head a column short of `w - 1`.
    let pad = w - 1 - width(&head);
    format!("{head}\u{2026}{}", " ".repeat(pad))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A CJK ideograph is one char and two columns — the whole reason this module exists.
    #[test]
    fn width_counts_columns_not_characters() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("日本語"), 6);
        assert_eq!("日本語".chars().count(), 3, "the char count disagrees, by design");
    }

    #[test]
    fn take_width_never_overruns_on_a_wide_boundary() {
        // Budget 3 cannot hold two ideographs (4 columns), and must not return one and a
        // half. It keeps one and stops.
        assert_eq!(take_width("日本語", 3), "日");
        assert!(width(&take_width("日本語", 3)) <= 3);
        assert_eq!(take_width("日本語", 4), "日本");
        assert_eq!(take_width("abcd", 3), "abc");
        assert_eq!(take_width("abc", 0), "");
    }

    #[test]
    fn take_width_end_keeps_the_tail_within_budget() {
        assert_eq!(take_width_end("日本語", 4), "本語");
        assert_eq!(take_width_end("abcd", 3), "bcd");
        assert!(width(&take_width_end("日本語", 3)) <= 3);
    }

    #[test]
    fn fit_exact_always_returns_exactly_w_columns() {
        for s in ["", "ab", "abcdefgh", "日本語", "a日b語c", "日本語日本語"] {
            for w in 0..12 {
                let got = fit_exact(s, w);
                assert_eq!(
                    width(&got),
                    w,
                    "fit_exact({s:?}, {w}) = {got:?} is {} columns, not {w}",
                    width(&got)
                );
            }
        }
    }
}
