//! `petri` — the Rust/ratatui reimplementation of the interactive dashboard.
//! Spec: `petri/SPEC.md`. S4 fills in the walking skeleton (real terminal, event
//! loop, poll timer, panic hook — petri/SPEC.md §9 S4); S5+ replace `app::render`
//! with the grouped Browser/Dashboard screens.

pub mod app;

/// Resolved default state-file path: `$HOME/.petridish/projects.json`. Mirrors
/// `swab::cli::default_state_path` — same reasoning (composed directly so tests
/// can override it without touching `HOME`).
pub fn default_state_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set");
    std::path::PathBuf::from(&home).join(".petridish").join("projects.json")
}

/// Entry point. Checks `state_path` exists *before* entering the alternate screen
/// (petri/SPEC.md §4 "Missing state file") — returns exit code 1 with the same
/// message `swab list`/`swab path` use if not. Otherwise enters the terminal,
/// runs the event loop (mtime poll, `q` quits), and restores the terminal on
/// every exit path including panic (a panic hook must be installed before the
/// alternate screen is entered). Returns the process exit code so this is
/// unit-testable without spawning a process, mirroring `swab::cli`'s handler
/// convention.
pub fn run(state_path: &std::path::Path) -> std::io::Result<u8> {
    todo!("S4: existence check, terminal init, event loop, poll timer, panic hook")
}
