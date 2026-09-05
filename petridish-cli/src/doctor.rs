//! `petridish doctor` — is the *install* intact?
//!
//! Deliberately distinct from `swab doctor`, which answers a different
//! question: is the *scanner* healthy (config parses, roots exist, state file
//! is fresh). Those checks are wired to `swab`'s own `Config` type, and this
//! crate does not depend on `swab` — that boundary is what stops the installer
//! reaching the state-file writer. Both commands print the same `ok:`/`fail:`
//! grammar so the output is learnable across the two.
//!
//! The hook check appears in both, on purpose: it is the one condition each
//! command genuinely needs to know about.

use crate::install::Layout;
use crate::plist::{MENUBAR_PLUGIN_FILENAME, PLIST_LABEL};
use crate::settings;
use petridish_core::schema::HOOK_EVENTS;
use std::io::Write;
use std::path::Path;

pub struct Check {
    pub key: &'static str,
    pub ok: bool,
    pub detail: String,
}

impl Check {
    fn pass(key: &'static str, detail: impl Into<String>) -> Self {
        Check {
            key,
            ok: true,
            detail: detail.into(),
        }
    }
    fn fail(key: &'static str, detail: impl Into<String>) -> Self {
        Check {
            key,
            ok: false,
            detail: detail.into(),
        }
    }
}

/// Extract the `<string>` immediately following the `ProgramArguments` array
/// opening — i.e. the `swab` path the plist will actually execute.
///
/// A deliberately small parser rather than a plist dependency: we wrote this
/// file from our own template, so the only question is which path is baked in.
fn program_path_from_plist(text: &str) -> Option<String> {
    let after = text.split("<array>").nth(1)?;
    let open = after.find("<string>")? + "<string>".len();
    let close = after[open..].find("</string>")?;
    Some(xml_unescape(after[open..open + close].trim()))
}

/// Undo `plist::xml_escape`.
///
/// Load-bearing rather than cosmetic: the plist stores `/Users/a&b/bin/swab` as
/// `/Users/a&amp;b/bin/swab`, and comparing that against the filesystem would
/// report a perfectly healthy install as broken — `doctor` telling a user to
/// re-run `install`, which would produce the identical plist and the identical
/// complaint. `&amp;` is expanded last so `&amp;lt;` does not become `<`.
fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Run every install-surface check. Pure over the filesystem it is handed, so
/// tests point it at a scratch layout.
pub fn checks(layout: &Layout, path_var: &str) -> Vec<Check> {
    let mut out = Vec::new();

    // 1. Binaries resolve, absolutely (D1/D2).
    for name in ["swab", "swab-hook", "petridish"] {
        out.push(match crate::paths::resolve_binary_in(name, path_var) {
            Ok(p) => Check::pass("binaries", format!("{name} -> {}", p.display())),
            Err(e) => Check::fail("binaries", e.to_string()),
        });
    }

    // 2. The plist exists, and the binary it names still does.
    //
    // This is the stale-plist failure: `brew upgrade` (or a `cargo install`
    // into a different prefix) can move `swab` out from under a plist that
    // still points at the old location, and launchd then runs nothing at all
    // while looking perfectly installed.
    let plist_path = layout.plist_path();
    if !plist_path.exists() {
        out.push(Check::fail(
            "plist",
            format!("missing: {}", plist_path.display()),
        ));
    } else {
        match std::fs::read_to_string(&plist_path) {
            Ok(text) => match program_path_from_plist(&text) {
                Some(prog) if Path::new(&prog).exists() => {
                    out.push(Check::pass("plist", format!("runs {prog}")))
                }
                Some(prog) => out.push(Check::fail(
                    "plist",
                    format!(
                        "points at {prog}, which no longer exists — re-run `petridish install`"
                    ),
                )),
                None => out.push(Check::fail("plist", "could not read ProgramArguments")),
            },
            Err(e) => out.push(Check::fail("plist", e.to_string())),
        }
    }

    // 3. Hook registration, per event — a machine installed before an event
    //    was added to HOOK_EVENTS is healthy-looking but partially wired.
    let settings_path = layout.settings_path();
    match crate::install::load_settings(&settings_path) {
        Ok(settings) => {
            let missing: Vec<&str> = HOOK_EVENTS
                .iter()
                .copied()
                .filter(|e| !settings::event_has_marker(&settings, e, settings::default_marker()))
                .collect();
            if missing.is_empty() {
                out.push(Check::pass(
                    "hook",
                    format!("all {} events registered", HOOK_EVENTS.len()),
                ));
            } else {
                out.push(Check::fail(
                    "hook",
                    format!(
                        "not registered for: {} — re-run `petridish install`",
                        missing.join(", ")
                    ),
                ));
            }
        }
        Err(e) => out.push(Check::fail("hook", format!("{settings_path:?}: {e}"))),
    }

    // 4. config.toml.
    let config_path = layout.data_dir().join("config.toml");
    out.push(if config_path.exists() {
        Check::pass("config", config_path.display().to_string())
    } else {
        Check::fail("config", format!("missing: {}", config_path.display()))
    });

    // 5. The menu-bar plugin, only when the user wants one.
    if let Some(dir) = &layout.menubar_plugins_dir {
        let plugin = dir.join(MENUBAR_PLUGIN_FILENAME);
        out.push(if plugin.exists() {
            Check::pass("menubar", plugin.display().to_string())
        } else {
            Check::fail("menubar", format!("missing: {}", plugin.display()))
        });
    }

    out
}

/// Print the checks and return the process exit code.
pub fn report(checks: &[Check], out: &mut dyn Write) -> i32 {
    let mut failed = false;
    for c in checks {
        if !c.ok {
            failed = true;
        }
        let _ = writeln!(
            out,
            "{}: {} — {}",
            if c.ok { "ok" } else { "fail" },
            c.key,
            c.detail
        );
    }
    let _ = writeln!(
        out,
        "\nlaunchd job status: launchctl print gui/$(id -u)/{PLIST_LABEL}"
    );
    i32::from(failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plist;

    #[test]
    fn program_path_is_read_from_the_first_program_argument() {
        let text = plist::render_plist("/opt/homebrew/bin/swab", "/tmp/l.log", PLIST_LABEL);
        assert_eq!(
            program_path_from_plist(&text),
            Some("/opt/homebrew/bin/swab".to_string())
        );
    }

    /// `render_plist` XML-escapes what it writes, so reading it back without
    /// decoding compares `/Users/a&amp;b/...` against the filesystem and reports
    /// a healthy install as stale — sending the user to re-run `install`, which
    /// writes the identical plist and produces the identical complaint.
    #[test]
    fn the_program_path_is_xml_decoded_before_it_is_used_as_a_path() {
        let text = plist::render_plist("/Users/a&b/<bin>/swab", "/tmp/l.log", PLIST_LABEL);
        assert!(
            text.contains("&amp;"),
            "precondition: the plist really is escaped"
        );
        assert_eq!(
            program_path_from_plist(&text),
            Some("/Users/a&b/<bin>/swab".to_string())
        );
    }

    #[test]
    fn xml_unescape_expands_ampersand_last() {
        // Naive ordering turns `&amp;lt;` into `<`.
        assert_eq!(xml_unescape("&amp;lt;"), "&lt;");
        assert_eq!(xml_unescape("a &amp; &lt;b&gt;"), "a & <b>");
    }

    #[test]
    fn program_path_is_none_for_a_plist_with_no_array() {
        assert_eq!(program_path_from_plist("<plist></plist>"), None);
    }

    #[test]
    fn report_exits_nonzero_when_any_check_failed() {
        let mut buf = Vec::new();
        assert_eq!(report(&[Check::pass("a", "fine")], &mut buf), 0);
        let mut buf = Vec::new();
        let code = report(
            &[Check::pass("a", "fine"), Check::fail("b", "broken")],
            &mut buf,
        );
        assert_eq!(code, 1);
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("ok: a"), "{text}");
        assert!(text.contains("fail: b"), "{text}");
    }
}
