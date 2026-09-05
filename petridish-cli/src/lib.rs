//! `petridish` — the command a user runs first.
//!
//! Owns everything that wires the tool into the machine: the launchd job, the
//! Claude Code hook entries in `~/.claude/settings.json`, the xbar menu-bar
//! plugin, and the health check over all three. `swab` (the scanner) and
//! `petri` (the dashboard) stay as they are; this crate is deliberately not
//! allowed to depend on either, so it cannot reach the `projects.json` writer.
//!
//! Replaces the Python `petridish.installer` and `petridish.menubar`.

pub mod doctor;
pub mod error;
pub mod install;
pub mod launchd;
pub mod menubar;
pub mod paths;
pub mod plist;
pub mod settings;
pub mod shell;

#[cfg(test)]
pub mod testutil;
