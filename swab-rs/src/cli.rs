//! `swab-rs` console entry point. Mirrors the `scan`/`list`/`path`/`doctor`/`config`
//! subcommands of `src/petridish/cli.py` (NOT `dash` — that pulls in TUI rendering code
//! that is explicitly out of scope for this port).

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "swab-rs")]
pub struct Cli {
    /// Path to the state file (default: ~/.petridish/projects.json).
    #[arg(long, global = true)]
    pub state: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run a full tick and write the state file. Prints
    /// "scanned {n} projects in {ms}ms -> {path}". Rotates ~/.petridish/daemon.log past 5MB first.
    Scan,
    /// Read cached state only — never triggers a scan.
    List {
        #[arg(long)]
        bucket: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Resolve the best-matching project path by name/substring (enables `cd $(swab-rs path x)`).
    /// Tie-break by most recent last_activity_at.
    Path { query: String },
    /// Health checks: config loads, roots exist, state file present/fresh (<24h), hook
    /// marker found in ~/.claude/settings.json. Exit non-zero on any failure.
    Doctor,
    /// Print config file location + every Config field (name/default), sourced from the
    /// struct definition so it can't drift from config.rs.
    Config,
}

pub fn main() {
    todo!("R9: clap parse, dispatch to scan/list/path/doctor/config handlers wired to scan.rs/config.rs")
}
