//! Rendering systemd user service and timer units for Linux.
//!
//! Both templates are `include_str!`'d rather than shipped as data files. That
//! removes the entire class of problem of a template missing from the installed
//! artifact — it is now a compile error rather than a runtime one. It also keeps
//! the unit files *reviewable artifacts*: a human diffing
//! `~/.config/systemd/user/petridish-scan.service` against the file in this repo
//! sees the same document, which building the units programmatically would not
//! preserve.

const SERVICE_TEMPLATE: &str = include_str!("../resources/petridish-scan.service");
const TIMER_TEMPLATE: &str = include_str!("../resources/petridish-scan.timer");

/// systemd unit name (shared by both the service and timer).
pub const UNIT_NAME: &str = "petridish-scan";

/// Filename of the installed systemd user service unit.
pub const SERVICE_FILENAME: &str = "petridish-scan.service";

/// Filename of the installed systemd user timer unit.
pub const TIMER_FILENAME: &str = "petridish-scan.timer";

/// Quote `s` for a systemd unit-file value (`ExecStart=`), so it is taken as
/// exactly one word regardless of spaces, quotes, `$`, or `%`.
///
/// Unit files parse values with C-style/shell-like quoting (systemd.syntax(7)):
/// a double-quoted string suppresses word-splitting, and only `\` and `"` need
/// escaping inside it. `$` is deliberately escaped too — unlike a POSIX shell,
/// systemd expands `$FOO`/`${FOO}` *inside* double quotes (environment and
/// specifier expansion), so a literal `$` in a path must be neutralized or a
/// path like `/Users/x/$HOME/bin/swab` would be silently mis-substituted. The
/// escape sequence for a literal `$` is `$$` (doubling), matching the
/// convention used in make and most POSIX tools.
///
/// `%` gets the same doubling treatment for a different reason: systemd's own
/// specifier syntax (`%h` for the unit's home directory, `%%` for a literal
/// `%`, etc. — systemd.unit(5)) is expanded in `ExecStart=` independently of
/// shell-style quoting, so a path segment like `50%homebrew/bin/swab` would
/// have `%h` interpreted as a specifier rather than two literal characters.
///
/// Backslash must be escaped first, before quote and dollar, so that a literal
/// backslash does not accidentally consume a later escape's backslash. `%`
/// doubling is independent of the other three (no shared characters) and can
/// be applied in any order relative to them.
fn unit_quote(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "$$")
        .replace('%', "%%");
    format!("\"{escaped}\"")
}

/// Render the systemd service unit, substituting the absolute path to the `swab`
/// binary and the log file path.
///
/// The absolute path is required, not a nicety. A relative path or bare command
/// name would fail at runtime when systemd runs the service outside of an
/// interactive shell, and would not be discoverable until the timer first fires.
/// `swab_abspath` is quoted with [`unit_quote`] because `ExecStart=` parses its
/// value with the shell-like command-line syntax that needs it. `log_path`
/// deliberately is *not* run through `unit_quote`: `StandardOutput=append:PATH`
/// and `StandardError=append:PATH` are not word-split or quote-parsed at
/// all — systemd (`config_parse_exec_output` in `load-fragment.c`) takes
/// everything after `append:` up to the end of the line as the literal path,
/// so embedded spaces are already safe, and wrapping it in `unit_quote`'s
/// double quotes would instead insert two literal `"` characters into the
/// filename. The one thing that *is* interpreted for this directive is `%`
/// specifier expansion (`%h`, `%n`, ... via `unit_path_printf`), so only `%`
/// is escaped, by doubling it.
pub fn render_service(swab_abspath: &str, log_path: &str) -> String {
    SERVICE_TEMPLATE
        .replace("__SWAB_PATH__", &unit_quote(swab_abspath))
        .replace("__LOG_PATH__", &log_path.replace('%', "%%"))
}

/// Render the systemd timer unit.
///
/// The timer has no placeholders, so this function simply returns the static
/// template verbatim. It is kept for symmetry with `render_service` so callers
/// do not need to special-case "this one has no substitution".
pub fn render_timer() -> String {
    TIMER_TEMPLATE.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_substitutes_both_placeholders() {
        let out = render_service("/opt/homebrew/bin/swab", "/Users/x/.petridish/daemon.log");
        assert!(!out.contains("__SWAB_PATH__"));
        assert!(!out.contains("__LOG_PATH__"));
        assert!(out.contains("ExecStart=\"/opt/homebrew/bin/swab\" scan"));
        assert!(out.contains("StandardOutput=append:/Users/x/.petridish/daemon.log"));
        assert!(out.contains("StandardError=append:/Users/x/.petridish/daemon.log"));
    }

    #[test]
    fn service_quotes_a_path_containing_a_space() {
        let out = render_service("/Volumes/Dev Disk/bin/swab", "/tmp/l.log");
        assert!(
            out.contains("ExecStart=\"/Volumes/Dev Disk/bin/swab\" scan"),
            "path with space must be quoted:\n{out}"
        );
    }

    #[test]
    fn service_neutralizes_a_dollar_sign_in_the_path() {
        let out = render_service("/Users/x/$HOME/bin/swab", "/tmp/l.log");
        // The `$` must become `$$` so systemd doesn't expand it.
        assert!(
            out.contains("ExecStart=\"/Users/x/$$HOME/bin/swab\" scan"),
            "literal $ must be escaped as $$:\n{out}"
        );
        assert!(
            !out.contains("/Users/x/$HOME"),
            "bare unescaped $HOME must not appear:\n{out}"
        );
    }

    #[test]
    fn service_escapes_an_embedded_double_quote_and_backslash() {
        let out = render_service("/Users/x/a\"b\\c/swab", "/tmp/l.log");
        assert!(
            out.contains("ExecStart=\"/Users/x/a\\\"b\\\\c/swab\" scan"),
            "embedded quote and backslash must be escaped:\n{out}"
        );
    }

    #[test]
    fn unit_quote_escapes_backslash_before_quote_and_dollar() {
        // Backslash must be replaced first, or a literal backslash would
        // accidentally consume a later escape's backslash.
        assert_eq!(unit_quote("a\\b"), "\"a\\\\b\"");
        assert_eq!(unit_quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(unit_quote("a$b"), "\"a$$b\"");
        // Combined: this would be wrong if backslash-escape were not first.
        // If we escaped " or $ before \, then a path like `a\"b` would become
        // `a\\\"b` (the backslash escapes the quote instead of being literal),
        // which is incorrect.
        assert_eq!(unit_quote("a\\\"b"), "\"a\\\\\\\"b\"");
    }

    /// systemd expands `%h`/`%%`-style specifiers in `ExecStart=` independently
    /// of quoting, so a literal `%` must be doubled or a path segment like
    /// `50%homebrew` would have `%h` read as the home-directory specifier.
    #[test]
    fn unit_quote_doubles_a_percent_sign() {
        assert_eq!(unit_quote("a%b"), "\"a%%b\"");
        assert_eq!(unit_quote("50%homebrew"), "\"50%%homebrew\"");
    }

    #[test]
    fn service_neutralizes_a_percent_sign_in_the_path() {
        let out = render_service("/opt/50%homebrew/bin/swab", "/tmp/l.log");
        assert!(
            out.contains("ExecStart=\"/opt/50%%homebrew/bin/swab\" scan"),
            "literal % must be doubled to %%, or systemd reads %h as a specifier:\n{out}"
        );
        assert!(
            !out.contains("/opt/50%homebrew"),
            "bare unescaped % must not appear:\n{out}"
        );
    }

    /// `StandardOutput=append:PATH` is not word-split, so a space in the log
    /// path must survive unquoted — unlike `ExecStart=`'s path, which goes
    /// through `unit_quote`.
    #[test]
    fn service_leaves_a_space_in_the_log_path_unquoted() {
        let out = render_service("/bin/swab", "/home/Alice Smith/.petridish/daemon.log");
        assert!(
            out.contains("StandardOutput=append:/home/Alice Smith/.petridish/daemon.log"),
            "log path with a space must appear literally, without quotes:\n{out}"
        );
        assert!(
            !out.contains("append:\""),
            "append: paths are not quote-parsed; a literal quote would become part of the filename:\n{out}"
        );
    }

    /// `append:` paths still go through `unit_path_printf`, which expands `%`
    /// specifiers (`%h`, `%n`, ...), so a literal `%` must be doubled just
    /// like it is for `ExecStart=`.
    #[test]
    fn service_doubles_a_percent_sign_in_the_log_path() {
        let out = render_service("/bin/swab", "/opt/50%homebrew/daemon.log");
        assert!(
            out.contains("StandardOutput=append:/opt/50%%homebrew/daemon.log"),
            "literal % in the log path must be doubled to %%:\n{out}"
        );
    }

    #[test]
    fn render_timer_returns_the_static_template_verbatim() {
        let out = render_timer();
        assert!(out.contains("OnStartupSec=0"));
        assert!(out.contains("OnUnitActiveSec=60s"));
        assert!(out.contains("AccuracySec=1s"));
        assert!(out.contains("WantedBy=timers.target"));
        assert!(out.contains("Unit=petridish-scan.service"));
        assert!(!out.contains("__"), "leftover placeholder in:\n{out}");
    }

    #[test]
    fn service_has_no_placeholders_left() {
        let out = render_service("/bin/swab", "/tmp/l.log");
        assert!(!out.contains("__"), "leftover placeholder in:\n{out}");
    }
}
