//! `petridish` binary: `install`, `uninstall`, `doctor`, `menubar`.

use clap::{Parser, Subcommand};
use petridish_cli::doctor;
use petridish_cli::error::InstallError;
use petridish_cli::install::{self, Backend, Binaries, Layout};
use petridish_cli::launchd::RealLaunchctl;
use petridish_cli::menubar;
use petridish_cli::paths::{self, Platform};
use petridish_cli::systemd::RealSystemctl;
use petridish_core::schema::Radar;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "petridish",
    version,
    about = "Wire up petridish: the scan daemon (launchd on macOS, systemd on Linux), the Claude Code hook, and (macOS only) the menu bar."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Install the scan daemon (launchd on macOS, a systemd --user timer on
    /// Linux), the Claude Code hook, and (macOS only) the menu-bar plugin.
    Install {
        /// Where to write the xbar/SwiftBar plugin. Defaults to xbar's own
        /// directory; SwiftBar's is user-configured and cannot be guessed.
        #[arg(long)]
        menubar_plugins_dir: Option<PathBuf>,
        /// Skip the menu-bar plugin entirely.
        #[arg(long)]
        no_menubar_plugin: bool,
    },
    /// Remove everything `install` added, leaving `~/.petridish` untouched.
    Uninstall {
        #[arg(long)]
        menubar_plugins_dir: Option<PathBuf>,
        #[arg(long)]
        no_menubar_plugin: bool,
    },
    /// Check that the install is intact.
    Doctor {
        #[arg(long)]
        menubar_plugins_dir: Option<PathBuf>,
        #[arg(long)]
        no_menubar_plugin: bool,
        /// Emit the checks as a JSON array instead of the human-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Print menu-bar text (xbar format) for the current state file.
    ///
    /// Consumed by xbar/SwiftBar on macOS and the Cinnamon applet
    /// (integrations/cinnamon) on Linux.
    ///
    /// Always exits 0: xbar disables a plugin that errors, so every failure
    /// degrades to a visible placeholder instead.
    Menubar {
        /// Read this state file instead of `~/.petridish/projects.json`.
        #[arg(long)]
        state: Option<PathBuf>,
    },
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Build the `Layout` for `platform`. `menubar_dir`/`no_menubar` govern the xbar
/// *plugin-file* install step only — that stays macOS-only, since `petridish
/// install` doesn't manage anything for the Cinnamon applet (it's copied in by
/// hand). This says nothing about `petridish menubar` itself, which renders on
/// every platform. `menubar_plugins_dir` is unconditionally `None` on every
/// other platform.
fn layout(platform: Platform, menubar_dir: Option<PathBuf>, no_menubar: bool) -> Layout {
    let home = home();
    let backend = match platform {
        Platform::Macos => Backend::Launchd {
            launch_agents_dir: home.join("Library").join("LaunchAgents"),
            uid: unsafe { libc_getuid() },
        },
        Platform::Linux => Backend::Systemd {
            unit_dir: paths::default_systemd_user_dir(&home),
        },
    };
    let menubar_plugins_dir = match platform {
        Platform::Macos if !no_menubar => {
            Some(menubar_dir.unwrap_or_else(|| paths::default_menubar_plugins_dir(&home)))
        }
        _ => None,
    };
    Layout {
        claude_dir: home.join(".claude"),
        backend,
        menubar_plugins_dir,
        home,
    }
}

// `getuid()` without pulling in the `libc` crate for one call. The launchd
// domain is `gui/<uid>`, so this has to be the real uid, not a guess.
unsafe extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

fn resolve_binaries() -> Result<Binaries, InstallError> {
    Ok(Binaries {
        swab: paths::resolve_binary("swab")?,
        swab_hook: paths::resolve_binary("swab-hook")?,
        // `current_exe` rather than a PATH lookup: the plugin should point at
        // the binary the user actually invoked, which is what `installer.py`
        // did with `sys.executable`. Falls back to PATH if the exe path is
        // unavailable.
        petridish: std::env::current_exe()
            .or_else(|_| paths::resolve_binary("petridish").map_err(std::io::Error::other))?,
    })
}

fn run() -> Result<i32, InstallError> {
    match Cli::parse().command {
        Command::Install {
            menubar_plugins_dir,
            no_menubar_plugin,
        } => {
            let platform = paths::detect_platform(std::env::consts::OS)?;
            let layout = layout(platform, menubar_plugins_dir, no_menubar_plugin);
            let bins = resolve_binaries()?;
            // Both controllers are always passed; `install` picks which one it
            // actually calls from `layout.backend`, so there is no separate
            // platform match here to keep in sync with it.
            install::install(
                &layout,
                &bins,
                &RealLaunchctl,
                &RealSystemctl,
                &mut std::io::stdout(),
            )?;
            Ok(0)
        }
        Command::Uninstall {
            menubar_plugins_dir,
            no_menubar_plugin,
        } => {
            let platform = paths::detect_platform(std::env::consts::OS)?;
            let layout = layout(platform, menubar_plugins_dir, no_menubar_plugin);
            install::uninstall(
                &layout,
                &RealLaunchctl,
                &RealSystemctl,
                &mut std::io::stdout(),
                &mut std::io::stderr(),
            )?;
            Ok(0)
        }
        Command::Doctor {
            menubar_plugins_dir,
            no_menubar_plugin,
            json,
        } => {
            let os = std::env::consts::OS;
            // `doctor` degrades rather than refuses on a platform `install`
            // itself would reject (issue #25) — an unrecognised OS still gets
            // a best-effort systemd-shaped `Layout` so the hook/config/binary
            // checks below still run. `doctor::checks` re-derives whether `os`
            // is actually supported itself and reports the daemon check as a
            // `skip` rather than trusting this fallback for that one check —
            // see its doc comment.
            let platform = paths::detect_platform(os).unwrap_or(Platform::Linux);
            let layout = layout(platform, menubar_plugins_dir, no_menubar_plugin);
            let path_var = std::env::var("PATH").unwrap_or_default();
            let checks = doctor::checks(&layout, &path_var, os);
            if json {
                println!("{}", doctor::checks_to_json(&checks));
                return Ok(i32::from(checks.iter().any(|c| !c.ok)));
            }
            Ok(doctor::report(&checks, &mut std::io::stdout(), os))
        }
        Command::Menubar { state } => {
            // Renders on every platform. Issue #25 used to refuse off-macOS because
            // nothing on Linux could read xbar plugin text; the Cinnamon applet in
            // integrations/cinnamon/ is now exactly such a reader, so the refusal's
            // premise is gone. The xbar *plugin install* remains macOS-only — this is
            // only the pure text renderer.
            let path = state.unwrap_or_else(|| home().join(".petridish").join("projects.json"));
            let text = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<Radar>(&t).ok())
                .map(|radar| menubar::render_menubar(&radar))
                .unwrap_or_else(|| menubar::render_unavailable(&path.to_string_lossy()));
            println!("{text}");
            Ok(0)
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}
