//! `petridish` binary: `install`, `uninstall`, `doctor`, `menubar`.

use clap::{Parser, Subcommand};
use petridish_cli::doctor;
use petridish_cli::error::InstallError;
use petridish_cli::install::{self, Binaries, Layout};
use petridish_cli::launchd::RealLaunchctl;
use petridish_cli::menubar;
use petridish_cli::paths;
use petridish_core::schema::Radar;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "petridish",
    version,
    about = "Wire up petridish: the launchd daemon, the Claude Code hook, and the menu bar."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Install the launchd job, the Claude Code hook, and the menu-bar plugin.
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
    },
    /// Print xbar/SwiftBar plugin text for the current state file.
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

fn layout(menubar_dir: Option<PathBuf>, no_menubar: bool) -> Layout {
    let home = home();
    let menubar_plugins_dir = if no_menubar {
        None
    } else {
        Some(menubar_dir.unwrap_or_else(|| paths::default_menubar_plugins_dir(&home)))
    };
    Layout {
        claude_dir: home.join(".claude"),
        launch_agents_dir: home.join("Library").join("LaunchAgents"),
        uid: unsafe { libc_getuid() },
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
            paths::check_platform(std::env::consts::OS)?;
            let layout = layout(menubar_plugins_dir, no_menubar_plugin);
            let bins = resolve_binaries()?;
            install::install(&layout, &bins, &RealLaunchctl, &mut std::io::stdout())?;
            Ok(0)
        }
        Command::Uninstall {
            menubar_plugins_dir,
            no_menubar_plugin,
        } => {
            paths::check_platform(std::env::consts::OS)?;
            let layout = layout(menubar_plugins_dir, no_menubar_plugin);
            install::uninstall(
                &layout,
                &RealLaunchctl,
                &mut std::io::stdout(),
                &mut std::io::stderr(),
            )?;
            Ok(0)
        }
        Command::Doctor {
            menubar_plugins_dir,
            no_menubar_plugin,
        } => {
            let layout = layout(menubar_plugins_dir, no_menubar_plugin);
            let path_var = std::env::var("PATH").unwrap_or_default();
            let checks = doctor::checks(&layout, &path_var);
            Ok(doctor::report(&checks, &mut std::io::stdout()))
        }
        Command::Menubar { state } => {
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
