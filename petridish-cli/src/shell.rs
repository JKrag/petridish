//! Rendering a path so a shell reads it as exactly one word.
//!
//! Two things this crate writes are executed by a shell it does not control:
//! the `command` string in `~/.claude/settings.json`, which Claude Code runs,
//! and the generated xbar plugin wrapper. In both cases the value is a
//! filesystem path chosen by whoever installed the binaries, and a path is
//! allowed to contain spaces, quotes, `$`, and backticks.
//!
//! `/Volumes/Dev Disk/bin/swab-hook` is the realistic case — a second drive with
//! a space in its name — and unquoted it makes the shell try to run
//! `/Volumes/Dev`. The rest are rarer but not exotic on a machine whose owner
//! names their own directories.

/// Wrap `s` in single quotes so a POSIX shell takes it as one literal word.
///
/// Single quotes suppress *all* expansion, so this is safe for `$`, backticks,
/// double quotes, backslashes and whitespace alike. The one character that
/// cannot appear inside single quotes is a single quote, which is handled the
/// standard way: close the string, emit an escaped quote, reopen it.
pub fn single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_path_is_merely_wrapped() {
        assert_eq!(
            single_quote("/opt/homebrew/bin/swab-hook"),
            "'/opt/homebrew/bin/swab-hook'"
        );
    }

    /// The case that motivated this: a volume name with a space in it.
    #[test]
    fn a_path_with_spaces_becomes_one_word() {
        assert_eq!(
            single_quote("/Volumes/Dev Disk/bin/swab-hook"),
            "'/Volumes/Dev Disk/bin/swab-hook'"
        );
    }

    #[test]
    fn shell_metacharacters_are_inert_inside_single_quotes() {
        for raw in [
            "/Users/x/$HOME/bin/swab",
            "/Users/x/`whoami`/bin/swab",
            "/Users/x/a\"b/swab",
            "/Users/x/a\\b/swab",
            "/Users/x/a;rm -rf b/swab",
        ] {
            let q = single_quote(raw);
            assert!(q.starts_with('\''), "{q}");
            assert!(q.ends_with('\''), "{q}");
            // Nothing but the wrapping quotes was added or removed.
            assert_eq!(&q[1..q.len() - 1], raw, "{q}");
        }
    }

    /// A directory really can contain an apostrophe — `/Users/o'brien/bin`.
    #[test]
    fn an_embedded_single_quote_is_escaped_by_closing_and_reopening() {
        assert_eq!(
            single_quote("/Users/o'brien/bin/swab"),
            r"'/Users/o'\''brien/bin/swab'"
        );
    }
}
