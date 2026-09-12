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

#[derive(serde::Serialize)]
pub struct Check {
    pub key: &'static str,
    pub ok: bool,
    /// Issue #25: `true` for a check that does not apply on this platform (the launchd
    /// plist and the xbar/SwiftBar menu-bar plugin, on non-macOS) — reported as "skip",
    /// distinct from both a pass and a fail, so a Linux `doctor` run does not read as a
    /// permanently broken install.
    #[serde(default)]
    pub skipped: bool,
    pub detail: String,
}

impl Check {
    fn pass(key: &'static str, detail: impl Into<String>) -> Self {
        Check {
            key,
            ok: true,
            skipped: false,
            detail: detail.into(),
        }
    }
    fn fail(key: &'static str, detail: impl Into<String>) -> Self {
        Check {
            key,
            ok: false,
            skipped: false,
            detail: detail.into(),
        }
    }
    /// Not applicable on this platform — issue #25. `ok: true` so it never fails the
    /// install and never trips `main.rs`'s `checks.iter().any(|c| !c.ok)` exit code; the
    /// distinct `skipped` flag is what lets `report()` say "not applicable" rather than
    /// a bare, confusing "ok".
    fn skip(key: &'static str, detail: impl Into<String>) -> Self {
        Check {
            key,
            ok: true,
            skipped: true,
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

/// 6. Every binary reports the same version as this build.
///
/// `petridish doctor` is itself one of the four binaries, so its own
/// `CARGO_PKG_VERSION` is the reference. Any binary that reports a different
/// one was built or installed separately — most often an older `swab` left in a
/// prefix that a later `cargo install` did not replace — and the launchd job
/// may run the stale one, which is a silent divergence worth flagging.
///
/// A binary that cannot be resolved (the "binaries" check already reports that)
/// or that fails to answer `--version` (crashed, needs a flag, produced no
/// output) is treated as "unknown" and left out of the comparison: it is not
/// the same failure as being missing, and a flaky probe must not fail a healthy
/// install. If nothing could be checked at all we still fail rather than
/// silently omit the check.
///
/// Pure over what it is handed: it takes `path_var` and runs nothing global, so
/// the test points it at a scratch directory of fake binaries.
fn version_check(path_var: &str) -> Check {
    const BINARIES: [&str; 4] = ["swab", "swab-hook", "petridish", "petri"];
    let expected = env!("CARGO_PKG_VERSION");

    // name -> version, for the binaries that actually resolved and answered
    // `--version` with a parseable string. The version is owned so it can be
    // kept past the loop body (the borrow of the command's stdout otherwise
    // would not outlive the loop).
    let mut versions: Vec<(&'static str, String)> = Vec::new();
    for name in BINARIES.iter().copied() {
        let path = match crate::paths::resolve_binary_in(name, path_var) {
            Ok(p) => p,
            Err(_) => continue, // already reported by the "binaries" check
        };
        let output = match std::process::Command::new(&path).arg("--version").output() {
            Ok(o) => o,
            Err(_) => continue, // could not even spawn it
        };
        if !output.status.success() {
            continue;
        }
        let version = match String::from_utf8(output.stdout) {
            Ok(s) => match s.split_whitespace().last().map(str::trim) {
                Some(v) if !v.is_empty() => v.to_string(),
                _ => continue, // no token, or nothing but whitespace
            },
            Err(_) => continue,
        };
        versions.push((name, version));
    }

    if versions.is_empty() {
        return Check::fail(
            "version",
            "could not check versions — no binaries could be resolved or reported one",
        );
    }

    let disagreements: Vec<(&'static str, String)> = versions
        .iter()
        .filter(|(_, v)| v.as_str() != expected)
        .cloned()
        .collect();

    if disagreements.is_empty() {
        let rendered = versions
            .iter()
            .map(|(name, version)| format!("{name} {}", version))
            .collect::<Vec<_>>()
            .join(", ");
        return Check::pass("version", format!("{rendered} — all agree"));
    }

    let rendered = disagreements
        .iter()
        .map(|(name, version)| format!("{name} reports {version}"))
        .collect::<Vec<_>>()
        .join(", ");
    Check::fail(
        "version",
        format!(
            "{rendered}, expected {expected} (petridish) — binaries are out of sync, reinstall/upgrade to match"
        ),
    )
}

/// Run every install-surface check. Pure over the filesystem it is handed, so
/// tests point it at a scratch layout.
///
/// `os` takes the same "parameter, not `#[cfg(target_os)]`" shape as
/// `paths::check_platform` (issue #25): the plist and menu-bar-plugin checks below are
/// meaningless on a platform `install`/`uninstall` already refuse to touch (launchd,
/// `~/Library`), so they report [`Check::skip`] there instead of a permanent, unactionable
/// `fail` — CI compiles and tests this crate on Linux, and a compile-time gate would make
/// the Linux branch untestable wherever this happens to build.
pub fn checks(layout: &Layout, path_var: &str, os: &str) -> Vec<Check> {
    let mut out = Vec::new();

    // 1. Binaries resolve, absolutely (D1/D2). Platform-independent: `swab`/`petri` run
    //    on Linux too (issues #23/#24), so this stays a real check everywhere.
    for name in ["swab", "swab-hook", "petridish"] {
        out.push(match crate::paths::resolve_binary_in(name, path_var) {
            Ok(p) => Check::pass("binaries", format!("{name} -> {}", p.display())),
            Err(e) => Check::fail("binaries", e.to_string()),
        });
    }

    // 2. The plist exists, and the binary it names still does. launchd-only: nothing to
    //    check on a platform `install` never wrote a plist for in the first place.
    if os != "macos" {
        out.push(Check::skip(
            "plist",
            "not applicable on this platform — launchd is macOS-only",
        ));
    } else {
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
                        Presence::Present => {
                            out.push(Check::pass("plist", format!("runs {prog}")))
                        }
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

    // 5. The menu-bar plugin, only when the user wants one. xbar/SwiftBar-only: the
    //    default plugin directory (`paths::default_menubar_plugins_dir`) is a macOS path
    //    that never exists on Linux, so without this gate every Linux `doctor` run would
    //    report a permanent, unfixable "missing" here even though menu-bar is a macOS-only
    //    experiment (issue #25) that install never wrote a plugin for.
    if let Some(dir) = &layout.menubar_plugins_dir {
        if os != "macos" {
            out.push(Check::skip(
                "menubar",
                "not applicable on this platform — the xbar/SwiftBar menu bar is macOS-only",
            ));
        } else {
            let plugin = dir.join(MENUBAR_PLUGIN_FILENAME);
            out.push(check_path(
                "menubar",
                &plugin,
                " — run `petridish install`, or `--no-menubar-plugin` if you do not want one",
            ));
        }
    }

    // 6. Every binary reports the same version as this build.
    out.push(version_check(path_var));

    out
}

/// Print the checks and return the process exit code.
///
/// `os` gates the trailing `launchctl` hint the same way [`checks`] gates the plist/menubar
/// checks themselves (issue #25) — printing a launchd command on a platform that has no
/// launchd is not just unhelpful, it actively contradicts a `skip: plist` line two lines
/// above it.
pub fn report(checks: &[Check], out: &mut dyn Write, os: &str) -> i32 {
    let mut failed = false;
    let mut passed = 0;
    let mut skipped = 0;
    for c in checks {
        if c.skipped {
            skipped += 1;
        } else if c.ok {
            passed += 1;
        } else {
            failed = true;
        }
        let _ = writeln!(
            out,
            "{}: {} — {}",
            if c.skipped {
                "skip"
            } else if c.ok {
                "ok"
            } else {
                "fail"
            },
            c.key,
            c.detail
        );
    }
    // The denominator is checks that actually apply here — a skip is neither a pass nor a
    // fail, and folding it into "N/M passed" would either inflate a Linux run's pass count
    // or (worse) read as a failure. Reported separately instead.
    let applicable = checks.len() - skipped;
    let failed_count = applicable - passed;
    let skip_suffix = if skipped == 0 {
        String::new()
    } else {
        format!(", {skipped} not applicable")
    };
    if failed_count == 0 {
        let _ = writeln!(out, "{applicable}/{applicable} checks passed{skip_suffix}");
    } else {
        let _ = writeln!(
            out,
            "{passed}/{applicable} checks passed ({failed_count} failed{skip_suffix})"
        );
    }
    if os == "macos" {
        let _ = writeln!(
            out,
            "\nlaunchd job status: launchctl print gui/$(id -u)/{PLIST_LABEL}"
        );
    }
    i32::from(failed)
}

/// The checks as a pretty-printed JSON array, for `doctor --json` — the same
/// data the human-facing `report()` renders, without its grammar.
pub fn checks_to_json(checks: &[Check]) -> String {
    serde_json::to_string_pretty(checks).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plist;
    use std::path::PathBuf;

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
        assert_eq!(report(&[Check::pass("a", "fine")], &mut buf, "macos"), 0);
        let mut buf = Vec::new();
        let code = report(
            &[Check::pass("a", "fine"), Check::fail("b", "broken")],
            &mut buf,
            "macos",
        );
        assert_eq!(code, 1);
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("ok: a"), "{text}");
        assert!(text.contains("fail: b"), "{text}");
        assert!(text.contains("1/2 checks passed (1 failed)"), "{text}");
        assert!(text.contains("launchd job status"), "{text}");
    }

    #[test]
    fn report_all_passing_shows_the_all_passed_summary() {
        let checks: Vec<Check> = (0..5)
            .map(|i| Check::pass("c", format!("ok {i}")))
            .collect();
        let mut buf = Vec::new();
        assert_eq!(report(&checks, &mut buf, "macos"), 0);
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("5/5 checks passed"), "{text}");
        let summary = text.find("5/5 checks passed").unwrap();
        let launchd = text.find("launchd job status").unwrap();
        assert!(summary < launchd, "{text}");
    }

    // ═══ Issue #25: skip, not fail, for the plist/menubar checks on non-macOS. ═══

    #[test]
    fn report_omits_the_launchd_hint_on_non_macos() {
        let mut buf = Vec::new();
        report(&[Check::pass("a", "fine")], &mut buf, "linux");
        let text = String::from_utf8(buf).unwrap();
        assert!(
            !text.contains("launchd job status"),
            "a platform with no launchd must not be told to run launchctl: {text}"
        );
    }

    #[test]
    fn report_shows_skipped_checks_separately_from_the_pass_fail_count() {
        let checks = vec![
            Check::pass("a", "fine"),
            Check::skip("plist", "not applicable on this platform"),
        ];
        let mut buf = Vec::new();
        let code = report(&checks, &mut buf, "linux");
        assert_eq!(code, 0, "a skip must never fail the install");
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("skip: plist"), "{text}");
        assert!(
            text.contains("1/1 checks passed"),
            "the skipped check must not count toward the applicable total: {text}"
        );
        assert!(text.contains("not applicable"), "{text}");
    }

    #[test]
    fn checks_reports_plist_and_menubar_as_skip_not_fail_on_linux() {
        let tmp = TempDir::new("doctor_linux_skip");
        let layout = Layout {
            home: tmp.path.clone(),
            claude_dir: tmp.path.join(".claude"),
            launch_agents_dir: tmp.path.join("Library").join("LaunchAgents"),
            uid: 501,
            menubar_plugins_dir: Some(tmp.path.join("xbar-plugins")),
        };
        let checks = checks(&layout, "", "linux");

        let plist = checks.iter().find(|c| c.key == "plist").expect("plist");
        assert!(
            plist.skipped,
            "plist must be skipped on Linux: {}",
            plist.detail
        );
        assert!(plist.ok, "a skip must not fail the install");

        let menubar = checks.iter().find(|c| c.key == "menubar").expect("menubar");
        assert!(
            menubar.skipped,
            "menubar must be skipped on Linux, not report the macOS plugin path as missing: {}",
            menubar.detail
        );
        assert!(menubar.ok, "a skip must not fail the install");
    }

    #[test]
    fn checks_still_runs_plist_and_menubar_for_real_on_macos() {
        let tmp = TempDir::new("doctor_macos_real");
        let layout = Layout {
            home: tmp.path.clone(),
            claude_dir: tmp.path.join(".claude"),
            launch_agents_dir: tmp.path.join("Library").join("LaunchAgents"),
            uid: 501,
            menubar_plugins_dir: Some(tmp.path.join("xbar-plugins")),
        };
        let checks = checks(&layout, "", "macos");

        let plist = checks.iter().find(|c| c.key == "plist").expect("plist");
        assert!(!plist.skipped, "plist must be a real check on macOS");
        assert!(!plist.ok, "no plist was written in this scratch layout");

        let menubar = checks.iter().find(|c| c.key == "menubar").expect("menubar");
        assert!(!menubar.skipped, "menubar must be a real check on macOS");
        assert!(
            !menubar.ok,
            "no plugin file was written in this scratch layout"
        );
    }

    #[test]
    fn checks_to_json_round_trips_as_an_array_of_the_same_length() {
        let checks = vec![
            Check::pass("binaries", "/opt/homebrew/bin/swab"),
            Check::fail("plist", "missing: /tmp/l.plist"),
        ];
        let json = checks_to_json(&checks);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), checks.len());
        assert_eq!(parsed[0]["key"].as_str(), Some("binaries"));
        assert_eq!(parsed[0]["ok"].as_bool(), Some(true));
        assert_eq!(parsed[1]["ok"].as_bool(), Some(false));
    }

    /// A fake binary: a shell script with a shebang, made executable, that
    /// answers `--version` with `"<name> <version>"` and nothing else. A real
    /// binary is exactly this shape on the surface `doctor` probes, so a fake
    /// shell script is a faithful stand-in without pulling in a dependency.
    fn write_fake_binary(dir: &Path, name: &str, version: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(
            &p,
            format!("#!/bin/sh\nprintf '%s %s\\n' \"{name}\" \"{version}\"\n"),
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn version_check_passes_when_every_binary_agrees() {
        let tmp = TempDir::new("doctor_version_agree");
        let version = env!("CARGO_PKG_VERSION");
        for name in ["swab", "swab-hook", "petridish", "petri"] {
            write_fake_binary(&tmp.path, name, version);
        }
        let check = version_check(tmp.path.to_str().unwrap());
        assert!(check.ok, "expected pass, got fail: {}", check.detail);
        assert!(check.detail.contains("all agree"), "{}", check.detail);
        // The detail names every binary that agreed.
        for name in ["swab", "swab-hook", "petridish", "petri"] {
            assert!(
                check.detail.contains(name),
                "detail should name {name}: {}",
                check.detail
            );
        }
    }

    #[test]
    fn version_check_fails_and_names_the_binary_that_disagrees() {
        let tmp = TempDir::new("doctor_version_disagree");
        let version = env!("CARGO_PKG_VERSION");
        write_fake_binary(&tmp.path, "swab", version);
        write_fake_binary(&tmp.path, "swab-hook", version);
        write_fake_binary(&tmp.path, "petridish", version);
        write_fake_binary(&tmp.path, "petri", "1.0.0-beta.1");
        let check = version_check(tmp.path.to_str().unwrap());
        assert!(!check.ok, "expected fail");
        assert!(
            check.detail.contains("petri"),
            "should name the disagreeing binary: {}",
            check.detail
        );
        assert!(check.detail.contains("1.0.0-beta.1"), "{}", check.detail);
        assert!(
            check.detail.contains(version),
            "should name the expected version: {}",
            check.detail
        );
    }

    #[test]
    fn version_check_treats_a_binary_that_exits_nonzero_as_unknown() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new("doctor_version_nonzero");
        let version = env!("CARGO_PKG_VERSION");
        // swab refuses to answer --version.
        std::fs::write(tmp.path.join("swab"), "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(
            tmp.path.join("swab"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        for name in ["swab-hook", "petridish", "petri"] {
            write_fake_binary(&tmp.path, name, version);
        }
        let check = version_check(tmp.path.to_str().unwrap());
        // The three that did answer agree, so the check still passes — the
        // failing binary was skipped, not treated as a mismatch.
        assert!(
            check.ok,
            "a failing binary must not fail a healthy install: {}",
            check.detail
        );
    }

    #[test]
    fn version_check_treats_unparseable_output_as_unknown() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new("doctor_version_badoutput");
        let version = env!("CARGO_PKG_VERSION");
        // swab prints no version token at all.
        std::fs::write(tmp.path.join("swab"), "#!/bin/sh\nprintf ''\n").unwrap();
        std::fs::set_permissions(
            tmp.path.join("swab"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        for name in ["swab-hook", "petridish", "petri"] {
            write_fake_binary(&tmp.path, name, version);
        }
        let check = version_check(tmp.path.to_str().unwrap());
        assert!(
            check.ok,
            "unparseable output must be treated as unknown: {}",
            check.detail
        );
    }
}
