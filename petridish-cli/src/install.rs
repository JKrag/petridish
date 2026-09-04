//! Orchestration: `install` and `uninstall`.
//!
//! The one module that mutates state outside our own data directory —
//! `~/.claude/settings.json` (shared with other hook consumers) and
//! `~/Library/LaunchAgents`. Every settings edit is structural; see
//! `settings.rs`.
//!
//! Every path is a parameter. Nothing here reads `$HOME` or `getuid()`; `main`
//! does that once and passes the results down. That is what `installer.py` did,
//! and it is why these tests never mutate process-global state and can run in
//! parallel.

use crate::error::InstallError;
use crate::launchd::{self, Launchctl};
use crate::plist::{self, LEGACY_MENUBAR_PLUGIN_FILENAME, MENUBAR_PLUGIN_FILENAME, PLIST_LABEL};
use crate::settings;
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG_TOML: &str = "\
# petridish config — every field below is optional; these are the defaults.
# Uncomment and edit to override.
#
# roots = [\"~/repos\", \"~/learning\"]
# extra_paths = []
# author_patterns = [\"Jan.*Krag\"]
# author_since = \"3 years\"
# max_depth = 4
";

/// Everything `install`/`uninstall` need to know about where things live.
///
/// A struct rather than eight parameters, because every caller passes the same
/// set and tests build one scratch instance and reuse it.
pub struct Layout {
    pub home: PathBuf,
    pub claude_dir: PathBuf,
    pub launch_agents_dir: PathBuf,
    pub uid: u32,
    /// `None` means "do not touch the menu-bar plugin" (`--no-menubar-plugin`).
    pub menubar_plugins_dir: Option<PathBuf>,
}

impl Layout {
    pub fn data_dir(&self) -> PathBuf {
        self.home.join(".petridish")
    }
    pub fn settings_path(&self) -> PathBuf {
        self.claude_dir.join("settings.json")
    }
    pub fn backup_path(&self) -> PathBuf {
        self.data_dir().join("settings.json.backup")
    }
    pub fn plist_path(&self) -> PathBuf {
        self.launch_agents_dir.join(format!("{PLIST_LABEL}.plist"))
    }
}

/// Where the binaries live. Resolved once by the caller so tests can supply
/// scratch paths without a fake `PATH`.
pub struct Binaries {
    pub swab: PathBuf,
    pub swab_hook: PathBuf,
    pub petridish: PathBuf,
}

pub fn load_settings(path: &Path) -> Result<Value, InstallError> {
    if !path.exists() {
        return Ok(Value::Object(Default::default()));
    }
    let text = std::fs::read_to_string(path)?;
    if text.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    Ok(serde_json::from_str(&text)?)
}

/// Write via temp file + rename, so a reader never sees a half-written file.
///
/// The temp name **appends** `.tmp` rather than replacing the extension:
/// Python's `with_suffix(suffix + ".tmp")` produced `settings.json.tmp`, while
/// Rust's `Path::set_extension` would produce `settings.tmp`. It stays in the
/// same directory, which is what makes the rename atomic.
pub fn write_settings_atomic(path: &Path, value: &Value) -> Result<(), InstallError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    std::fs::write(&tmp, settings::serialize_settings(value))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Copy `settings.json` aside, **once**.
///
/// A second install must not overwrite a clean pre-install backup with a dirty
/// one that already contains our entries. This is a safety artifact for the
/// human and is never read back automatically: restoring it on uninstall would
/// silently discard every unrelated edit made since (other consumers
/// reinstalling, `/model` writes, hand edits) — ARCHITECTURE.md §8.3 D4.
pub fn backup_settings(settings_path: &Path, backup_path: &Path) -> Result<(), InstallError> {
    if backup_path.exists() {
        return Ok(());
    }
    if let Some(parent) = backup_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if settings_path.exists() {
        std::fs::copy(settings_path, backup_path)?;
    } else {
        std::fs::write(backup_path, "")?;
    }
    Ok(())
}

/// Write `config.toml` only if absent. `true` iff it wrote one.
pub fn write_default_config(data_dir: &Path) -> Result<bool, InstallError> {
    let path = data_dir.join("config.toml");
    if path.exists() {
        return Ok(false);
    }
    std::fs::write(&path, DEFAULT_CONFIG_TOML)?;
    Ok(true)
}

/// Write a file and mark it executable. Used only for the menu-bar plugin: a
/// plist is data launchd reads and must never be `+x`, whereas an xbar plugin
/// is executed and must be.
fn write_executable(path: &Path, content: &str) -> Result<(), InstallError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

pub fn install(
    layout: &Layout,
    bins: &Binaries,
    ctl: &dyn Launchctl,
    out: &mut dyn Write,
) -> Result<(), InstallError> {
    let data_dir = layout.data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let wrote = write_default_config(&data_dir)?;
    let _ = writeln!(
        out,
        "{} config: {}",
        if wrote { "wrote" } else { "kept existing" },
        data_dir.join("config.toml").display()
    );

    let settings_path = layout.settings_path();
    let backup_path = layout.backup_path();
    backup_settings(&settings_path, &backup_path)?;

    let current = load_settings(&settings_path)?;
    let hook = bins.swab_hook.to_string_lossy();
    match settings::add_hook_entries(&current, &hook, settings::default_marker()) {
        Some(updated) => {
            write_settings_atomic(&settings_path, &updated)?;
            let _ = writeln!(out, "added hook entries to {}", settings_path.display());
        }
        None => {
            let _ = writeln!(
                out,
                "hook already installed in settings.json for every event; left untouched"
            );
        }
    }
    let _ = writeln!(out, "pre-install backup kept at {}", backup_path.display());

    let plist_path = layout.plist_path();
    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &plist_path,
        plist::render_plist(
            &bins.swab.to_string_lossy(),
            &data_dir.join("daemon.log").to_string_lossy(),
            PLIST_LABEL,
        ),
    )?;
    launchd::load_job(&plist_path, layout.uid, PLIST_LABEL, ctl)?;
    let _ = writeln!(out, "launchd job loaded: {}", plist_path.display());

    // Always re-rendered, so a moved binary is picked up by a plain reinstall.
    if let Some(dir) = &layout.menubar_plugins_dir {
        let plugin_path = dir.join(MENUBAR_PLUGIN_FILENAME);
        write_executable(
            &plugin_path,
            &plist::render_menubar_wrapper(&bins.petridish.to_string_lossy()),
        )?;
        let _ = writeln!(out, "menubar plugin installed: {}", plugin_path.display());

        // An upgrade from the Python install leaves its plugin behind, and two
        // plugins would both render into the menu bar.
        let legacy = dir.join(LEGACY_MENUBAR_PLUGIN_FILENAME);
        if legacy.exists() {
            std::fs::remove_file(&legacy)?;
            let _ = writeln!(out, "removed superseded plugin: {}", legacy.display());
        }
    }

    Ok(())
}

pub fn uninstall(
    layout: &Layout,
    ctl: &dyn Launchctl,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<(), InstallError> {
    launchd::unload_job(PLIST_LABEL, layout.uid, ctl, warn);

    let plist_path = layout.plist_path();
    if plist_path.exists() {
        std::fs::remove_file(&plist_path)?;
        let _ = writeln!(out, "removed {}", plist_path.display());
    }

    let settings_path = layout.settings_path();
    let current = load_settings(&settings_path)?;
    if settings::has_marker(&current, settings::default_marker()) {
        let cleaned = settings::remove_marker_entries(&current, settings::default_marker());
        write_settings_atomic(&settings_path, &cleaned)?;
        let _ = writeln!(out, "removed hook entries from {}", settings_path.display());
    } else {
        let _ = writeln!(
            out,
            "no hook entries found in settings.json; nothing to remove"
        );
    }

    if let Some(dir) = &layout.menubar_plugins_dir {
        let mut removed_any = false;
        for name in [MENUBAR_PLUGIN_FILENAME, LEGACY_MENUBAR_PLUGIN_FILENAME] {
            let path = dir.join(name);
            if path.exists() {
                std::fs::remove_file(&path)?;
                let _ = writeln!(out, "removed {}", path.display());
                removed_any = true;
            }
        }
        if !removed_any {
            let _ = writeln!(
                out,
                "nothing to remove: menubar plugin not found at given directory"
            );
        }
    }

    // D6: user data survives uninstall untouched.
    let data_dir = layout.data_dir();
    let backup_path = layout.backup_path();
    if backup_path.exists() {
        let _ = writeln!(
            out,
            "pre-install backup left in place at {} (not restored automatically)",
            backup_path.display()
        );
    }
    let _ = writeln!(
        out,
        "user data left in place at {} (config.toml, projects.json, events.ndjson)",
        data_dir.display()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launchd::recording::RecordingLaunchctl;
    use crate::settings;
    use crate::testutil::TempDir;
    use serde_json::json;

    struct Fixture {
        _tmp: TempDir,
        layout: Layout,
        bins: Binaries,
    }

    fn fixture(tag: &str, with_menubar: bool) -> Fixture {
        let tmp = TempDir::new(tag);
        let home = tmp.path.join("home");
        let claude_dir = home.join(".claude");
        let launch_agents_dir = home.join("Library/LaunchAgents");
        let plugins = tmp.path.join("xbar-plugins");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::create_dir_all(&launch_agents_dir).unwrap();
        std::fs::create_dir_all(&plugins).unwrap();

        let bindir = tmp.path.join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        Fixture {
            layout: Layout {
                home,
                claude_dir,
                launch_agents_dir,
                uid: 501,
                menubar_plugins_dir: with_menubar.then_some(plugins),
            },
            bins: Binaries {
                swab: bindir.join("swab"),
                swab_hook: bindir.join("swab-hook"),
                petridish: bindir.join("petridish"),
            },
            _tmp: tmp,
        }
    }

    /// A settings.json with other consumers already in it, as every real
    /// machine has.
    fn seed_settings(f: &Fixture) {
        std::fs::write(
            f.layout.settings_path(),
            settings::serialize_settings(&json!({
                "model": "opus",
                "hooks": {
                    "PreToolUse": [
                        {"hooks": [{"type": "command", "command": "/usr/local/bin/rtk-hook"}]}
                    ]
                }
            })),
        )
        .unwrap();
    }

    fn run_install(f: &Fixture) -> Result<String, InstallError> {
        let ctl = RecordingLaunchctl::new(&[0]);
        let mut out = Vec::new();
        install(&f.layout, &f.bins, &ctl, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    fn run_uninstall(f: &Fixture) -> String {
        let ctl = RecordingLaunchctl::new(&[0]);
        let mut out = Vec::new();
        let mut warn = Vec::new();
        uninstall(&f.layout, &ctl, &mut out, &mut warn).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn read_settings(f: &Fixture) -> Value {
        load_settings(&f.layout.settings_path()).unwrap()
    }

    #[test]
    fn install_writes_config_plist_hooks_and_plugin() {
        let f = fixture("install_full", true);
        seed_settings(&f);
        run_install(&f).unwrap();

        assert!(f.layout.data_dir().join("config.toml").exists());
        assert!(f.layout.plist_path().exists());
        assert!(f.layout.backup_path().exists());

        let plist_text = std::fs::read_to_string(f.layout.plist_path()).unwrap();
        assert!(plist_text.contains(&f.bins.swab.to_string_lossy().to_string()));

        let settings = read_settings(&f);
        for event in petridish_core::schema::HOOK_EVENTS {
            assert!(
                settings::event_has_marker(&settings, event, settings::default_marker()),
                "{event} not registered"
            );
        }

        let plugin = f
            .layout
            .menubar_plugins_dir
            .as_ref()
            .unwrap()
            .join(MENUBAR_PLUGIN_FILENAME);
        assert!(plugin.exists());
        assert!(
            std::fs::read_to_string(&plugin)
                .unwrap()
                .contains(&f.bins.petridish.to_string_lossy().to_string()),
            "the plugin must embed the absolute binary path"
        );
    }

    /// The plist is data launchd reads; the plugin is a script xbar executes.
    /// Getting either mode wrong is silent.
    #[test]
    fn the_plugin_is_executable_and_the_plist_is_not() {
        use std::os::unix::fs::PermissionsExt;
        let f = fixture("install_modes", true);
        run_install(&f).unwrap();

        let plugin = f
            .layout
            .menubar_plugins_dir
            .as_ref()
            .unwrap()
            .join(MENUBAR_PLUGIN_FILENAME);
        let plugin_mode = std::fs::metadata(&plugin).unwrap().permissions().mode();
        assert!(
            plugin_mode & 0o111 != 0,
            "plugin must be +x, got {plugin_mode:o}"
        );

        let plist_mode = std::fs::metadata(f.layout.plist_path())
            .unwrap()
            .permissions()
            .mode();
        assert!(
            plist_mode & 0o111 == 0,
            "plist must not be +x, got {plist_mode:o}"
        );
    }

    #[test]
    fn a_second_install_reports_nothing_to_do_and_changes_nothing() {
        let f = fixture("install_idempotent", true);
        seed_settings(&f);
        run_install(&f).unwrap();
        let after_first = std::fs::read_to_string(f.layout.settings_path()).unwrap();

        let output = run_install(&f).unwrap();
        assert!(output.contains("left untouched"), "{output}");
        assert_eq!(
            std::fs::read_to_string(f.layout.settings_path()).unwrap(),
            after_first
        );
    }

    #[test]
    fn install_never_disturbs_another_consumers_entries() {
        let f = fixture("install_siblings", true);
        seed_settings(&f);
        let before = read_settings(&f);
        run_install(&f).unwrap();
        let after = read_settings(&f);

        assert_eq!(after["model"], before["model"]);
        assert_eq!(
            after["hooks"]["PreToolUse"][0], before["hooks"]["PreToolUse"][0],
            "the pre-existing group must survive verbatim and stay first"
        );
    }

    /// The backup exists for a human to inspect. Overwriting it on a second
    /// install would replace a clean pre-install snapshot with a dirty one that
    /// already contains our own entries — exactly when it stops being useful.
    #[test]
    fn the_backup_is_taken_once_and_never_overwritten() {
        let f = fixture("install_backup", true);
        seed_settings(&f);
        run_install(&f).unwrap();
        let first = std::fs::read_to_string(f.layout.backup_path()).unwrap();
        assert!(
            !first.contains("swab-hook"),
            "backup must predate our edits"
        );

        run_install(&f).unwrap();
        assert_eq!(
            std::fs::read_to_string(f.layout.backup_path()).unwrap(),
            first
        );
    }

    #[test]
    fn install_with_no_existing_settings_file_still_works() {
        let f = fixture("install_no_settings", false);
        run_install(&f).unwrap();
        let settings = read_settings(&f);
        for event in petridish_core::schema::HOOK_EVENTS {
            assert!(settings::event_has_marker(
                &settings,
                event,
                settings::default_marker()
            ));
        }
        assert_eq!(
            std::fs::read_to_string(f.layout.backup_path()).unwrap(),
            "",
            "with nothing to back up, the backup is an empty marker file"
        );
    }

    #[test]
    fn install_skips_the_plugin_entirely_when_no_directory_is_given() {
        let f = fixture("install_no_plugin", false);
        let output = run_install(&f).unwrap();
        assert!(!output.contains("menubar plugin"), "{output}");
    }

    /// Upgrading from the Python install: its `.py` plugin must go, or the user
    /// ends up with two plugins rendering into the menu bar, one of them
    /// pointing at a package that no longer exists.
    #[test]
    fn install_clears_the_superseded_python_plugin() {
        let f = fixture("install_legacy", true);
        let dir = f.layout.menubar_plugins_dir.as_ref().unwrap();
        let legacy = dir.join(LEGACY_MENUBAR_PLUGIN_FILENAME);
        std::fs::write(&legacy, "#!/usr/bin/python3\n").unwrap();

        run_install(&f).unwrap();
        assert!(!legacy.exists(), "the old .py plugin must be removed");
        assert!(dir.join(MENUBAR_PLUGIN_FILENAME).exists());
    }

    #[test]
    fn uninstall_removes_our_wiring_and_leaves_everything_else() {
        let f = fixture("uninstall_full", true);
        seed_settings(&f);
        let before = std::fs::read_to_string(f.layout.settings_path()).unwrap();
        run_install(&f).unwrap();
        run_uninstall(&f);

        assert!(!f.layout.plist_path().exists());
        assert!(
            !f.layout
                .menubar_plugins_dir
                .as_ref()
                .unwrap()
                .join(MENUBAR_PLUGIN_FILENAME)
                .exists()
        );
        assert!(!settings::has_marker(
            &read_settings(&f),
            settings::default_marker()
        ));
        // D6: user data survives.
        assert!(f.layout.data_dir().join("config.toml").exists());
        assert!(f.layout.backup_path().exists());

        // And the file is byte-for-byte what we found.
        assert_eq!(
            std::fs::read_to_string(f.layout.settings_path()).unwrap(),
            before
        );
    }

    /// The D4 regression guard: uninstall must remove only *marked* entries,
    /// never restore the backup wholesale. Restoring would silently revert
    /// every unrelated change made between install and uninstall.
    #[test]
    fn uninstall_does_not_restore_the_backup_over_unrelated_later_edits() {
        let f = fixture("uninstall_no_restore", false);
        seed_settings(&f);
        run_install(&f).unwrap();

        // A change made after install, by someone else entirely.
        let mut current = read_settings(&f);
        current["model"] = json!("sonnet");
        current["hooks"]["Stop"]
            .as_array_mut()
            .unwrap()
            .push(json!({"hooks": [{"type": "command", "command": "/opt/newcomer/hook"}]}));
        write_settings_atomic(&f.layout.settings_path(), &current).unwrap();

        run_uninstall(&f);
        let after = read_settings(&f);
        assert_eq!(after["model"], json!("sonnet"), "a later edit was reverted");
        assert!(
            after["hooks"]["Stop"]
                .as_array()
                .unwrap()
                .iter()
                .any(|g| g.to_string().contains("newcomer")),
            "another consumer's later addition was lost: {after}"
        );
    }

    #[test]
    fn uninstall_on_a_never_installed_machine_is_a_quiet_no_op() {
        let f = fixture("uninstall_clean", true);
        let output = run_uninstall(&f);
        assert!(output.contains("nothing to remove"), "{output}");
        assert!(output.contains("no hook entries found"), "{output}");
    }

    #[test]
    fn uninstall_also_clears_a_leftover_python_plugin() {
        let f = fixture("uninstall_legacy", true);
        let dir = f.layout.menubar_plugins_dir.as_ref().unwrap();
        let legacy = dir.join(LEGACY_MENUBAR_PLUGIN_FILENAME);
        std::fs::write(&legacy, "#!/usr/bin/python3\n").unwrap();
        run_uninstall(&f);
        assert!(!legacy.exists());
    }

    #[test]
    fn write_settings_atomic_leaves_no_tmp_file_beside_the_target() {
        let f = fixture("atomic_write", false);
        let path = f.layout.settings_path();
        write_settings_atomic(&path, &json!({"a": 1})).unwrap();
        assert!(path.exists());
        assert!(
            !PathBuf::from(format!("{}.tmp", path.display())).exists(),
            "the temp file must be renamed away, not left behind"
        );
    }

    #[test]
    fn load_settings_treats_missing_and_blank_files_as_empty() {
        let f = fixture("load_settings", false);
        let path = f.layout.claude_dir.join("nope.json");
        assert_eq!(load_settings(&path).unwrap(), json!({}));
        std::fs::write(&path, "   \n").unwrap();
        assert_eq!(load_settings(&path).unwrap(), json!({}));
    }

    #[test]
    fn an_existing_config_toml_is_never_overwritten() {
        let f = fixture("config_kept", false);
        std::fs::create_dir_all(f.layout.data_dir()).unwrap();
        std::fs::write(
            f.layout.data_dir().join("config.toml"),
            "roots = [\"/x\"]\n",
        )
        .unwrap();
        let output = run_install(&f).unwrap();
        assert!(output.contains("kept existing config"), "{output}");
        assert_eq!(
            std::fs::read_to_string(f.layout.data_dir().join("config.toml")).unwrap(),
            "roots = [\"/x\"]\n"
        );
    }
}
