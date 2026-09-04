//! Rendering the launchd plist and the xbar plugin wrapper.
//!
//! Both templates are `include_str!`'d rather than shipped as data files. That
//! removes the entire class of problem `installer.py` needed `importlib.resources`
//! and a `[tool.setuptools.package-data]` stanza to solve — a template missing
//! from the installed artifact is now a compile error rather than a runtime one.
//! It also keeps the plist a *reviewable artifact*: a human diffing
//! `~/Library/LaunchAgents/com.petridish.daemon.plist` against the file in this
//! repo sees the same document, which building the XML programmatically would
//! not preserve.

const PLIST_TEMPLATE: &str = include_str!("../resources/com.petridish.daemon.plist");
const MENUBAR_WRAPPER_TEMPLATE: &str = include_str!("../resources/petridish_menubar.30s.sh");

/// launchd job label. Load-bearing: `uninstall` and `doctor` address the job by
/// this exact string, and it is also the plist's filename stem.
pub const PLIST_LABEL: &str = "com.petridish.daemon";

/// Filename of the installed xbar plugin.
///
/// `.30s` is xbar's refresh-interval convention (every 30 seconds), not
/// decoration. The extension is `.sh` because the file *is* shell now; xbar
/// dispatches on the shebang rather than the extension, so `.py` would have
/// worked and been actively misleading.
pub const MENUBAR_PLUGIN_FILENAME: &str = "petridish_menubar.30s.sh";

/// The pre-Rust plugin filename, kept only so `uninstall` can clear it.
///
/// Someone upgrading from the Python install has this file sitting in their
/// xbar plugins directory; without removing it they would end up running two
/// menu-bar plugins, one of which now points at a package that no longer
/// exists.
pub const LEGACY_MENUBAR_PLUGIN_FILENAME: &str = "petridish_menubar.30s.py";

/// Escape text for XML *element content*: `&`, `<`, `>`, with `&` first.
///
/// Quotes are deliberately not escaped, matching `xml.sax.saxutils.escape`
/// called with no `entities` argument. That is correct here rather than merely
/// faithful: every substituted value lands inside `<string>` content, never in
/// an attribute, so a literal quote needs no escaping.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn render_plist(swab_abspath: &str, log_path: &str, label: &str) -> String {
    PLIST_TEMPLATE
        .replace("__LABEL__", &xml_escape(label))
        .replace("__SWAB_PATH__", &xml_escape(swab_abspath))
        .replace("__LOG_PATH__", &xml_escape(log_path))
}

/// Render the xbar plugin wrapper, embedding an **absolute** path to the
/// `petridish` binary.
///
/// The absolute path is required, not a nicety (ARCHITECTURE.md §8.3 D1/D2).
/// xbar is launched by the GUI session and inherits launchd's environment, not
/// an interactive shell's — neither `~/.cargo/bin` nor `/opt/homebrew/bin` is
/// reliably on its `PATH`, and it never sources `.zshrc`. A wrapper saying
/// `exec petridish menubar` passes every terminal test and then shows nothing
/// on a real machine. This is the same reason the Python plugin substituted an
/// absolute interpreter path into its shebang.
/// The substituted path is shell-quoted, not dropped into `BIN="..."` raw. A
/// path may contain `$`, a backtick, a double quote or a backslash, all of which
/// the shell would otherwise interpret — silently running the wrong thing rather
/// than failing loudly.
pub fn render_menubar_wrapper(petridish_abspath: &str) -> String {
    MENUBAR_WRAPPER_TEMPLATE.replace(
        "__PETRIDISH_PATH__",
        &crate::shell::single_quote(petridish_abspath),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_substitutes_all_three_placeholders() {
        let out = render_plist(
            "/opt/homebrew/bin/swab",
            "/Users/x/.petridish/daemon.log",
            PLIST_LABEL,
        );
        assert!(!out.contains("__LABEL__"));
        assert!(!out.contains("__SWAB_PATH__"));
        assert!(!out.contains("__LOG_PATH__"));
        assert!(out.contains("<string>com.petridish.daemon</string>"));
        assert!(out.contains("<string>/opt/homebrew/bin/swab</string>"));
        assert!(out.contains("<string>/Users/x/.petridish/daemon.log</string>"));
        assert!(out.contains("<string>scan</string>"));
    }

    /// A real home directory can contain `&`, and an unescaped one produces a
    /// plist launchd silently refuses to parse.
    #[test]
    fn plist_escapes_xml_metacharacters_in_paths() {
        let out = render_plist("/Users/a&b/<bin>/swab", "/tmp/l.log", PLIST_LABEL);
        assert!(
            out.contains("<string>/Users/a&amp;b/&lt;bin&gt;/swab</string>"),
            "{out}"
        );
        assert!(!out.contains("/Users/a&b/"));
    }

    #[test]
    fn xml_escape_expands_ampersands_once_not_twice() {
        assert_eq!(xml_escape("a & <b>"), "a &amp; &lt;b&gt;");
        // `&` must be replaced first, or `&lt;` becomes `&amp;lt;`.
        assert_eq!(xml_escape("<"), "&lt;");
    }

    #[test]
    fn plist_has_no_placeholders_left_and_stays_well_formed_xml() {
        let out = render_plist("/bin/swab", "/tmp/l.log", PLIST_LABEL);
        assert!(out.starts_with("<?xml version=\"1.0\""));
        assert!(out.trim_end().ends_with("</plist>"));
        assert!(!out.contains("__"), "leftover placeholder in:\n{out}");
    }

    #[test]
    fn menubar_wrapper_embeds_an_absolute_path_and_never_a_bare_command() {
        let out = render_menubar_wrapper("/opt/homebrew/bin/petridish");
        assert!(out.starts_with("#!/bin/sh\n"));
        assert!(out.contains("/opt/homebrew/bin/petridish"));
        assert!(!out.contains("__PETRIDISH_PATH__"));
        // The guard that keeps xbar from disabling the plugin outright when the
        // binary is gone.
        assert!(
            out.contains("-x"),
            "missing the executable-exists guard:\n{out}"
        );
    }

    /// The wrapper is shell. A path containing `$` or a backtick dropped into
    /// `BIN="..."` would be expanded, running something other than what was
    /// installed.
    #[test]
    fn the_wrapper_shell_quotes_the_path_it_embeds() {
        let out = render_menubar_wrapper("/Users/x/$HOME `id`/bin/petridish");
        assert!(
            out.contains("BIN='/Users/x/$HOME `id`/bin/petridish'"),
            "path must be single-quoted:\n{out}"
        );
        assert!(
            !out.contains("BIN=\""),
            "a double-quoted path would still expand $ and backticks:\n{out}"
        );
    }
}
