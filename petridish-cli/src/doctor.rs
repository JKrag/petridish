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

/// Whether a path is there — with "we could not tell" kept distinct from "no".
///
/// `Path::exists()` collapses those two into `false`, which makes `doctor` give
/// confidently wrong advice: a caller that lacks permission to read
/// `~/Library/LaunchAgents` is told the plist is *missing* and to re-run
/// `petridish install`, which will not help and can be repeated indefinitely.
/// Observed on a genuinely healthy install, from a shell without the relevant
/// macOS privacy permission.
///
/// The distinction was always available — `std::fs::metadata` reports
/// `PermissionDenied` separately from `NotFound` — it was simply being discarded.
enum Presence {
    Present,
    Absent,
    /// Could not determine, with the reason.
    Unknown(String),
}

fn presence(path: &Path) -> Presence {
    match std::fs::metadata(path) {
        Ok(_) => Presence::Present,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Presence::Absent,
        Err(e) => Presence::Unknown(e.to_string()),
    }
}

/// The common shape: pass when present, fail when absent, and report
/// "cannot check" — still a failure, but a differently actionable one — when we
/// were not allowed to look.
fn check_path(key: &'static str, path: &Path, missing_hint: &str) -> Check {
    match presence(path) {
        Presence::Present => Check::pass(key, path.display().to_string()),
        Presence::Absent => Check::fail(key, format!("missing: {}{missing_hint}", path.display())),
        Presence::Unknown(why) => Check::fail(
            key,
            format!(
                "cannot check {} — {why}. A permissions problem, not a broken install; re-running `petridish install` will not change it.",
                path.display()
            ),
        ),
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
    match presence(&plist_path) {
        Presence::Absent => out.push(Check::fail(
            "plist",
            format!(
                "missing: {} — run `petridish install`",
                plist_path.display()
            ),
        )),
        Presence::Unknown(why) => out.push(Check::fail(
            "plist",
            format!(
                "cannot check {} — {why}. A permissions problem, not a broken install; re-running `petridish install` will not change it.",
                plist_path.display()
            ),
        )),
        Presence::Present => match std::fs::read_to_string(&plist_path) {
            Ok(text) => match program_path_from_plist(&text) {
                Some(prog) => match presence(Path::new(&prog)) {
                    Presence::Present => out.push(Check::pass("plist", format!("runs {prog}"))),
                    Presence::Absent => out.push(Check::fail(
                        "plist",
                        format!("points at {prog}, which no longer exists — re-run `petridish install`"),
                    )),
                    Presence::Unknown(why) => out.push(Check::fail(
                        "plist",
                        format!("points at {prog}, which could not be checked — {why}"),
                    )),
                },
                None => out.push(Check::fail("plist", "could not read ProgramArguments")),
            },
            Err(e) => out.push(Check::fail("plist", e.to_string())),
        },
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
    out.push(check_path(
        "config",
        &config_path,
        " — run `petridish install`",
    ));

    // 5. The menu-bar plugin, only when the user wants one.
    if let Some(dir) = &layout.menubar_plugins_dir {
        let plugin = dir.join(MENUBAR_PLUGIN_FILENAME);
        out.push(check_path(
            "menubar",
            &plugin,
            " — run `petridish install`, or `--no-menubar-plugin` if you do not want one",
        ));
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

    use crate::testutil::TempDir;

    #[test]
    fn presence_tells_present_from_absent() {
        let tmp = TempDir::new("doctor_presence");
        let f = tmp.path.join("here");
        std::fs::write(&f, "x").unwrap();
        assert!(matches!(presence(&f), Presence::Present));
        assert!(matches!(
            presence(&tmp.path.join("not-here")),
            Presence::Absent
        ));
    }

    /// The bug this fixes, reproduced rather than asserted: a directory we are
    /// not allowed to traverse must read as "cannot tell", never as "absent".
    /// Getting this wrong made `doctor` report a healthy install as broken and
    /// tell the user to re-run `install`, which cannot help.
    #[test]
    fn an_unreadable_path_is_unknown_not_absent() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new("doctor_denied");
        let locked = tmp.path.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        let target = locked.join("plist");
        std::fs::write(&target, "x").unwrap();

        // Remove traversal permission on the parent directory.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let verdict = presence(&target);
        // Restore before asserting, so a failure still leaves a removable dir.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        match verdict {
            Presence::Unknown(_) => {}
            Presence::Absent => panic!("permission denied must not read as absent"),
            Presence::Present => panic!("unexpectedly readable — is this running as root?"),
        }
    }

    #[test]
    fn check_path_reports_a_permission_problem_as_such() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new("doctor_checkpath");
        let locked = tmp.path.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        let target = locked.join("config.toml");
        std::fs::write(&target, "x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let check = check_path("config", &target, " — run `petridish install`");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(!check.ok);
        assert!(check.detail.contains("cannot check"), "{}", check.detail);
        assert!(
            !check.detail.contains("missing"),
            "must not claim the file is missing: {}",
            check.detail
        );
        // The advice must not send the user round a loop that cannot help.
        assert!(
            check.detail.contains("permissions problem"),
            "{}",
            check.detail
        );
    }

    #[test]
    fn check_path_still_reports_a_genuinely_missing_file_as_missing() {
        let tmp = TempDir::new("doctor_missing");
        let check = check_path("config", &tmp.path.join("nope.toml"), " — run install");
        assert!(!check.ok);
        assert!(check.detail.contains("missing"), "{}", check.detail);
        assert!(check.detail.contains("run install"), "{}", check.detail);
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
