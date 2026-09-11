//! `petri` — the Rust/ratatui reimplementation of the interactive dashboard.
//! Spec: `petri/SPEC.md`. S4 fills in the walking skeleton (real terminal,
//! event loop, poll timer, panic hook — petri/SPEC.md §9 S4); S5+ replace
//! `app::render` with the grouped Browser/Dashboard screens.

pub mod app;
pub mod browser;
pub mod dashboard;
pub mod exec;
pub mod feed;
pub mod focus;
pub mod help;
pub mod picker;
pub mod prefs;
pub mod theme;
pub mod tools;
pub mod width;
use crate::prefs::{LastScreen, Prefs};

/// Row count for the Browser's `Shift`-style fast-jump keys (`J`/`K`). 10 is
/// the fixed "about ten lines" jump — deliberately NOT tied to viewport
/// height (unlike `PageUp`/`PageDown`, which jump exactly one screenful):
/// this is the small/predictable hop, that one is the big/screen-relative one.
const BROWSER_FAST_JUMP: i32 = 10;

/// Resolved default state-file path: `$HOME/.petridish/projects.json`. Mirrors
/// `swab::cli::default_state_path` — same reasoning (composed directly so tests
/// can override it without touching `HOME`).
pub fn default_state_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set");
    std::path::PathBuf::from(&home)
        .join(".petridish")
        .join("projects.json")
}

// ---------------------------------------------------------------------------
// `--mini` (issue #31, `PLAN-focus-panel.md` T7) — argv contract and project
// resolution. Scaffolded in Phase C, implemented in T7. Deliberately in `lib.rs`
// rather than `main.rs`: integration tests can only reach the lib crate, and the
// argv contract below is the trap this scaffold exists to pin down.
// ---------------------------------------------------------------------------

/// Which project a `--mini` pane is pointed at, as the *user* named it — never an index.
///
/// An index into `radar.projects` is not a target, it is an answer, and it expires: the
/// scanner re-sorts on every scan (`SPEC.md` §4.3), so a pane left in a corner for days
/// would silently start showing a different project. `resolve_mini` turns one of these
/// into an index, and T7 calls it **every tick**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MiniTarget {
    /// No argument: the project the terminal is standing in
    /// (`PROPOSAL-focus-panel.md` §8.1).
    Cwd,
    /// `petri --mini <PATH|NAME>`: a pinned pane. Path first, then name — see
    /// `resolve_mini`.
    Pinned(String),
}

/// The one-line usage summary. Extracted to a const because it was previously written out
/// by hand at each `parse_args` error site, and a usage string that exists in more than one
/// place is one that eventually disagrees with itself — `--help` (issue #42) would have been
/// the third copy.
pub const USAGE: &str = "usage: petri [STATE_PATH] [--mini [PATH|NAME]]";

/// The full `--help` text (issue #42).
///
/// Deliberately only covers the *command line*. The in-app key bindings have their own
/// surface — the `?` popup (`help.rs`), which generates the action half from
/// `tools::registry()` and so cannot drift from what is actually bound. Duplicating the
/// keys here would create exactly the second copy `USAGE` exists to avoid, so this points
/// at `?` instead of restating it.
pub const HELP: &str = "\
petri — a terminal dashboard for the petridish project radar.

usage:
  petri [STATE_PATH] [--mini [PATH|NAME]]

arguments:
  STATE_PATH        Read the radar from this file instead of the default
                    (~/.petridish/projects.json). Read-only — petri never
                    writes it; `swab scan` is the only writer.

options:
  --mini [PATH|NAME]  Run as a single-project pane instead of the full
                      dashboard. With no operand, shows the project the
                      terminal is standing in. The operand is matched as a
                      path first, then as a project name.
  -h, --help          Print this help and exit.
  -V, --version       Print the version and exit.

notes:
  `--mini`'s operand is the argument immediately following it, and only if it
  does not start with `-`. So `petri --mini state.json` pins a project named
  `state.json` rather than reading a state file from it.

  Press `?` inside petri for the key bindings.";

/// The parsed command line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CliArgs {
    /// `--help` / `-h` was given: print [`HELP`] and exit 0, ignoring everything else.
    /// Outranks `version` — see `parse_args`' rule 0.
    pub help: bool,
    /// `--version` / `-V` was given: print the version and exit, ignoring everything else.
    pub version: bool,
    /// The state-file path override (the long-standing first-positional test hook the PTY
    /// suite depends on). `None` means `default_state_path()`.
    pub state_path: Option<std::path::PathBuf>,
    /// `--mini` and its optional target. `None` means the normal full dashboard.
    pub mini: Option<MiniTarget>,
}

/// Parse `argv` (**without** argv\[0\]) into `CliArgs`. Scaffold: `unimplemented!()`
/// until T7.
///
/// # The contract, decided here because `petri --mini foo` is genuinely ambiguous
///
/// `petri` has no `clap` (`SPEC.md` §10 does not list it) and its first positional
/// argument is already spoken for as a state-file path — a documented test hook the PTY
/// suite depends on. So `petri --mini foo` could mean "mini, pinned to foo" or "mini,
/// state file foo", and something has to choose. The rules, in order:
///
/// 0. **`--help` or `-h` in any position wins over everything, including `--version`**
///    (issue #42). Convention across GNU and BSD tools alike: when someone asks both what
///    the tool is and what version it is, they are lost, and the help is the answer that
///    helps. Short-circuits for the same reason rule 1 does.
/// 1. **`--version` or `-V` in any position wins** over everything else, and nothing else
///    is parsed. (Today's
///    `main.rs` only looks at argv\[1\]; this widens it, which is a superset of the shipped
///    behaviour rather than a change to it.)
/// 2. **`--mini`'s operand is the argument immediately following it**, and only if that
///    argument exists and does not start with `-`. So `petri --mini --version` is a
///    version request with no target, and `petri --mini` at the end of the line is
///    `MiniTarget::Cwd`.
/// 3. **The state path is the first remaining positional** — the first argument that is
///    neither a flag nor consumed as `--mini`'s operand. This is what keeps
///    `petri state.json` and `petri state.json --mini` working. Note rule 2 outranks it:
///    in `petri --mini state.json` the operand is `state.json`, a *pin*, not a path.
/// 4. A second positional, a second `--mini`, or any unrecognised `-`-leading argument is
///    an error, returned as the message to print. Failing loudly beats silently ignoring
///    an argument the user clearly meant something by.
pub fn parse_args(argv: &[String]) -> Result<CliArgs, String> {
    // Rule 0, checked before rule 1 so `petri --help --version` prints the help: a user
    // who asked for both is lost, and the help is the answer that helps.
    if argv.iter().any(|a| a == "--help" || a == "-h") {
        return Ok(CliArgs {
            help: true,
            ..CliArgs::default()
        });
    }

    // Rule 1, and it short-circuits: `--version` anywhere means the rest of the line is
    // never parsed, so `petri --mini --version` cannot fail on the operand it does not have.
    if argv.iter().any(|a| a == "--version" || a == "-V") {
        return Ok(CliArgs {
            version: true,
            ..CliArgs::default()
        });
    }

    let mut out = CliArgs::default();
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];

        if arg == "--mini" {
            if out.mini.is_some() {
                return Err("--mini given twice: a pane shows one project".to_string());
            }
            // Rule 2: the operand is the *immediately following* argument, and only if it
            // is not itself a flag. This is what outranks the state-path positional, so
            // `petri --mini state.json` is a pin rather than a path.
            match argv.get(i + 1).filter(|a| !a.starts_with('-')) {
                Some(target) => {
                    out.mini = Some(MiniTarget::Pinned(target.clone()));
                    i += 2;
                }
                None => {
                    out.mini = Some(MiniTarget::Cwd);
                    i += 1;
                }
            }
            continue;
        }

        if arg.starts_with('-') {
            return Err(format!("unrecognised argument {arg}\n{USAGE}"));
        }

        // Rule 3, then rule 4: the first bare positional is the state path, and a second
        // one is an error rather than a silently discarded argument.
        if out.state_path.is_some() {
            return Err(format!(
                "unexpected extra argument {arg}: the state path is given once\n{USAGE}"
            ));
        }
        out.state_path = Some(std::path::PathBuf::from(arg));
        i += 1;
    }

    Ok(out)
}

/// Why a `--mini` target could not be turned into a project.
///
/// `Display` renders the two-line, name-the-problem-and-the-fix message
/// `PROPOSAL-focus-panel.md` §8.3 specifies, following `SPEC.md` §4.4's precedent. T7
/// prints it **before** entering the alternate screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MiniError {
    /// A path (the cwd, or a pinned path) that no project in `projects.json` contains.
    NotAProject(std::path::PathBuf),
    /// A pinned name matching no project.
    UnknownName(String),
}

impl std::fmt::Display for MiniError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MiniError::NotAProject(path) => write!(
                f,
                "{} is not in projects.json\n\
                 run 'swab scan' to pick it up, or 'petri --mini <name>' to pin another project",
                crate::dashboard::abbreviate_home(&path.display().to_string())
            ),
            MiniError::UnknownName(name) => write!(
                f,
                "no project named '{name}' in projects.json\n\
                 run 'swab scan' if it is new, or 'petri --mini <path>' to name it by path"
            ),
        }
    }
}

/// Resolve a `--mini` target against the current `radar`, returning an index into
/// `radar.projects` that is valid **for this tick only**. Scaffold: `unimplemented!()`
/// until T7.
///
/// `cwd` is a parameter rather than a `current_dir()` read inside, per `CLAUDE.md`'s
/// parameter-over-environment rule — the same reason `petridish-cli` takes `home`.
///
/// # Resolution order
///
/// - `MiniTarget::Cwd` → the **path walk** below, against `cwd`.
/// - `MiniTarget::Pinned(s)` → the path walk against `s` first; if that finds nothing, the
///   **name match** below; otherwise `UnknownName` (`NotAProject` is reserved for the
///   cwd/path reading, so the message can be specific about which of the two the user
///   meant).
///
/// # The name match, when a name is not unique
///
/// Names are not unique — `SelectionAnchor`'s doc comment records a live fleet with three
/// projects called `smoke` — so `--mini <NAME>` has to say what it does with several
/// candidates. It takes the **most recently active** one: greatest `last_activity_at`,
/// `None` last, ties broken by `path` ascending.
///
/// That is not a coin flip dressed up as a rule. It is `swab`'s *own* ordering
/// (`scan.rs`'s sort: `last_activity_at` desc, `None` last, then name) — the same
/// judgement the Dashboard's own top-of-list already encodes — so "the `smoke` you mean"
/// is the `smoke` you were last working in, which is the answer a pane you just opened
/// wants essentially every time. Erroring out instead would be technically safer and
/// practically useless.
///
/// It is re-derived here rather than read off `radar.projects`' order, even though the
/// scanner writes the file in exactly this order today: relying on the order would make
/// this silently follow any future change to the writer's sort, and the tie-break on
/// `path` (unique) rather than `name` (identical by construction, here) is what keeps the
/// answer stable when two same-named projects have no activity at all.
///
/// The cost, stated plainly: a pane pinned by an ambiguous name can move to the other
/// project when that one becomes the more recently active. Accepted deliberately — the
/// unambiguous fix is to pin by path, and anyone with two same-named projects live at once
/// is in the corner case they built for themselves.
///
/// # The path walk, and why it is not a port of the scanner's `resolve_root`
///
/// Compare the path and each of its ancestors, deepest first, against the `path` field of
/// every project, and take the first hit. Deepest-first matters: a project nested inside
/// another project's tree must resolve to itself, not to its parent.
///
/// `PROPOSAL-focus-panel.md` §8.1 says to run the cwd "through the same `resolve_root()`
/// the scanner uses". `petri` cannot do that literally — it does not depend on `swab`
/// (which is protected) and has no `gix` — and it does not need to: `projects.json`
/// already contains `resolve_root`'s answers, one per project. Walking up and matching
/// reads that answer back rather than recomputing it, which is *stronger* than a port
/// against the invariant §8.1 is protecting (two different answers to "which project is
/// this directory"), because there is only ever one implementation. It also subsumes the
/// "walk up to the git toplevel first" nicety for free: `src/sensors/` finds the project
/// because the project root is one of its ancestors, with no `.git` probe at all.
///
/// No `~` expansion and no symlink canonicalisation: the shell does the former, and the
/// latter would make the answer depend on the filesystem, which no test could pin.
pub fn resolve_mini(
    radar: &petridish_core::schema::Radar,
    target: &MiniTarget,
    cwd: &std::path::Path,
) -> Result<usize, MiniError> {
    match target {
        MiniTarget::Cwd => {
            project_containing(radar, cwd).ok_or_else(|| MiniError::NotAProject(cwd.to_path_buf()))
        }
        MiniTarget::Pinned(s) => {
            // Path first, then name — so a pin that *is* a path can never be shadowed by a
            // project that happens to be named after it.
            if let Some(idx) = project_containing(radar, std::path::Path::new(s)) {
                return Ok(idx);
            }
            project_named(radar, s).ok_or_else(|| MiniError::UnknownName(s.clone()))
        }
    }
}

/// The project whose root is `path` or an ancestor of it, deepest match first.
///
/// Ancestry is component-wise (`Path::ancestors`), never a string prefix: `/repos/foo-old`
/// is not inside `/repos/foo`, and a `starts_with` on the raw strings says it is.
///
/// Deepest-first is what makes a project checked out inside another project's tree — a
/// worktree under its parent, a vendored repo — resolve to itself rather than to the
/// enclosing root.
fn project_containing(
    radar: &petridish_core::schema::Radar,
    path: &std::path::Path,
) -> Option<usize> {
    for ancestor in path.ancestors() {
        if let Some(idx) = radar
            .projects
            .iter()
            .position(|p| std::path::Path::new(&p.path) == ancestor)
        {
            return Some(idx);
        }
    }
    None
}

/// The project called `name`, or the most recently active one when several share it.
///
/// `swab`'s own ordering (`scan.rs`'s sort): `last_activity_at` descending with `None`
/// last, ties broken by path. `Option`'s derived `Ord` already puts `None` below every
/// `Some`, so a plain descending compare gives the None-last half for free.
///
/// **The tie-break compares paths case-insensitively first**, then byte-wise for
/// determinism when two paths differ only in case. Plain byte order is not the
/// "alphabetically first path" anyone reading a fleet list would name: real roots live
/// under mixed-case directories (`~/repos/JKrag/...`), and ASCII puts every capital ahead
/// of every lowercase letter, so a raw compare sorts `repos/JKrag/lantern` ahead of
/// `repos/aaa-first/lantern`. macOS paths are case-insensitive anyway, which is the fleet
/// this tool watches.
fn project_named(radar: &petridish_core::schema::Radar, name: &str) -> Option<usize> {
    radar
        .projects
        .iter()
        .enumerate()
        .filter(|(_, p)| p.name == name)
        .min_by(|(_, a), (_, b)| {
            b.last_activity_at
                .cmp(&a.last_activity_at)
                .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
                .then_with(|| a.path.cmp(&b.path))
        })
        .map(|(idx, _)| idx)
}

/// Read and deserialize the state file. The error message is promoted to
/// `io::Error` so the caller can unify JSON parse failures with IO errors.
fn read_state_file(path: &std::path::Path) -> std::io::Result<petridish_core::schema::Radar> {
    let text = std::fs::read_to_string(path)?;
    serde_json::from_str(&text)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

/// `petri --mini` (issue #31): one project, whole screen, no list.
///
/// Structurally `run`'s sibling rather than a mode inside it — the two share the
/// pre-alternate-screen preflight and the panic hook, but nothing about the key handling
/// or the reload bookkeeping, and folding a screen with no cursor into `poll_loop`'s
/// screen/picker/help/notice state machine would add branches to the hot key path for no
/// shared behaviour.
///
/// **Every failure that can be diagnosed before the terminal is touched, is**
/// (`SPEC.md` §4.4): the missing state file, an unreadable one, and — the one this mount
/// adds — a target that resolves to no project. Discovering "that isn't a project" only
/// after the alternate screen swallowed the message is the failure §4.4 exists to prevent.
pub fn run_mini(
    state_path: &std::path::Path,
    target: &MiniTarget,
    cwd: &std::path::Path,
) -> std::io::Result<u8> {
    if !state_path.exists() {
        eprintln!(
            "no state file at {}; run 'swab scan' first",
            state_path.display()
        );
        return Ok(1);
    }

    let radar = match read_state_file(state_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("petri: initial state read failed: {e}");
            return Ok(1);
        }
    };

    // The resolution preflight. Its *answer* is thrown away deliberately: an index is
    // valid for one tick only (`resolve_mini`'s doc comment), so the loop re-resolves the
    // `MiniTarget` every frame. What this call buys is the error, on a normal stderr.
    if let Err(e) = resolve_mini(&radar, target, cwd) {
        eprintln!("petri --mini: {e}");
        return Ok(1);
    }

    let prefs = prefs::load(&prefs::default_prefs_path());

    let _guard = TerminalGuard::enter()?;
    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;
    install_panic_hook();

    // `_guard`'s `Drop` restores the terminal here whether the `?` above or below fires or
    // the function falls through to `Ok(exit_code)` (issue #45).
    let exit_code = mini_poll_loop(state_path, &mut terminal, radar, target, cwd, &prefs)?;

    Ok(exit_code)
}

/// `--mini`'s event loop. Same poll cadence and same quiet-tick rule as `poll_loop` (draw
/// only on a real event or an mtime change, so an idle pane leaves the output stream
/// still), and the same feed bookkeeping via `absorb_snapshot` — the `Recent` rung is a
/// slice of the same activity feed the Dashboard shows.
///
/// `q` and `Esc` both quit. There is nothing for `Esc` to dismiss here, and a pane whose
/// only binding is a letter is a trap in a tmux split.
fn mini_poll_loop(
    state_path: &std::path::Path,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    initial: petridish_core::schema::Radar,
    target: &MiniTarget,
    cwd: &std::path::Path,
    prefs: &Prefs,
) -> std::io::Result<u8> {
    let mut feed = crate::feed::FeedState::seeded(&initial);
    let mut last_good = Some(initial);
    let mut last_mtime = std::fs::metadata(state_path)
        .ok()
        .and_then(|m| m.modified().ok());

    render_mini_frame(terminal, &last_good, target, cwd, &feed, prefs);

    loop {
        let event_ready =
            crossterm::event::poll(std::time::Duration::from_secs(1)).unwrap_or(false);
        if event_ready && let Ok(crossterm::event::Event::Key(key)) = crossterm::event::read() {
            match key.code {
                crossterm::event::KeyCode::Char('q') | crossterm::event::KeyCode::Esc => {
                    return Ok(0);
                }
                // Tool shortcuts (issue #64), minimal on purpose: `--mini` has
                // no picker and no notice pane, so only the unambiguous case
                // is wired up — a resolution that would need either
                // (`Ambiguous`/`NoTool`/`NoTarget`) is silently a no-op, the
                // same as pressing an unbound key already is. Full parity
                // with the Dashboard/Browser (picker, notices) is issue #65.
                crossterm::event::KeyCode::Char(c) => {
                    let registry = crate::tools::registry();
                    if let Some(r) = last_good.as_ref()
                        && let Ok(idx) = resolve_mini(r, target, cwd)
                        && let Some(project) = r.projects.get(idx)
                        && let Some(action) = registry.iter().find(|a| a.key == c)
                        && let crate::tools::Resolution::Ready(launch) =
                            resolve_action(action, project, prefs)
                    {
                        launch_now(terminal, &launch, std::path::Path::new(&project.path));
                    }
                }
                _ => {}
            }
        }

        let new_mtime = std::fs::metadata(state_path)
            .ok()
            .and_then(|m| m.modified().ok());
        let mtime_changed = match (&last_mtime, new_mtime) {
            (Some(prev), Some(now)) => *prev != now,
            _ => false,
        };

        if mtime_changed {
            // A failed read is swallowed on purpose: under the alternate screen there is
            // nowhere to print, and the pane keeps showing the last good snapshot —
            // degrade in place, same as `poll_loop`'s mid-run reads. `swab` writes via
            // temp-file + atomic rename, so a torn read here is a transient the next tick
            // fixes, not a state worth reporting.
            if let Ok(r) = read_state_file(state_path) {
                last_good = absorb_snapshot(&mut feed, last_good.take(), r);
            }
        }

        if event_ready || mtime_changed {
            render_mini_frame(terminal, &last_good, target, cwd, &feed, prefs);
        }

        last_mtime = new_mtime;
    }
}

/// Draw one `--mini` frame, re-resolving the target first.
///
/// **The resolution happens here, every frame, and its result is never held across one**
/// (`SPEC.md` §4.3): the scanner re-sorts `radar.projects` on every scan, so a cached
/// index would silently start pointing at a different project — and a corner pane is the
/// worst place for that, since there is no list on screen to make the swap visible.
///
/// A target that stops resolving mid-run (the project left the fleet) renders the same
/// message the preflight would have printed, in-pane. The alternative — exiting out from
/// under the user because one scan dropped a project — is worse for something pinned in a
/// split for days.
fn render_mini_frame(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    radar: &Option<petridish_core::schema::Radar>,
    target: &MiniTarget,
    cwd: &std::path::Path,
    feed: &crate::feed::FeedState,
    prefs: &Prefs,
) {
    let Some(r) = radar else { return };
    let resolved = resolve_mini(r, target, cwd);
    let _ = terminal.draw(|frame| {
        let area = frame.area();
        match resolved {
            Ok(idx) => {
                let ctx = crate::focus::FocusCtx {
                    radar: r,
                    target: crate::focus::FocusTarget::Project(idx),
                    now: chrono::Utc::now(),
                    feed: Some(feed),
                    prefs,
                };
                crate::focus::render_mini(frame, area, &ctx);
            }
            Err(ref e) => {
                let text: Vec<ratatui::text::Line<'static>> = e
                    .to_string()
                    .lines()
                    .map(|l| {
                        ratatui::text::Line::from(ratatui::text::Span::styled(
                            l.to_string(),
                            ratatui::style::Style::default().fg(crate::theme::DIM),
                        ))
                    })
                    .collect();
                frame.render_widget(ratatui::widgets::Paragraph::new(text), area);
            }
        }
    });
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
    // Step 1: existence check — BEFORE any terminal mutation. The PTY test
    // captures combined stdout/stderr output and asserts the shared swab
    // message shape, so this must run with no screen touched yet.
    if !state_path.exists() {
        eprintln!(
            "no state file at {}; run 'swab scan' first",
            state_path.display()
        );
        return Ok(1);
    }

    // Step 1.5: the initial state read and prefs load ALSO happen before any
    // terminal mutation, for the same reason as the existence check above —
    // any eprintln! warning either one produces (a corrupt state file on
    // first read, a missing/corrupt petri.toml) must land on a normal,
    // non-alternate-screen stderr. Doing this after EnterAlternateScreen was
    // a real bug, not a theoretical one: on the very first run (no
    // petri.toml yet, the expected shape for every new user) the "missing
    // preferences file" warning collided with the header's first draw and
    // visibly corrupted it — caught by smoke-testing against real data.
    let initial_radar = match read_state_file(state_path) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("petri: initial state read failed: {e}");
            None
        }
    };
    let prefs = prefs::load(&prefs::default_prefs_path());

    // Step 2: enter alternate screen + raw mode via `TerminalGuard`, which restores both on
    // drop — including on every `?` below, not just the normal exit path (issue #45).
    let _guard = TerminalGuard::enter()?;

    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;

    // Step 3: install a panic hook that leaves the alternate screen and
    // disables raw mode *before* unwinding further. The spec calls a panic
    // that leaves the user's terminal in raw mode "a v1 blocker, not a polish
    // item". Installed here so the alt screen + raw mode are covered even if
    // setup itself panics. The previous hook is captured and re-invoked so the
    // panic message still reaches stderr (in restored, non-raw mode).
    install_panic_hook();

    // Step 4: event loop. Step 5 (terminal restore) is `_guard`'s `Drop`, which fires here
    // whether this `?` returns early or the function falls through to `Ok(exit_code)` below.
    let exit_code = poll_loop(state_path, &mut terminal, initial_radar, prefs)?;

    Ok(exit_code)
}

/// Raw mode + alternate screen, torn down on every path out — including an `Err` from
/// whatever runs between `enter()` and the guard's drop (issue #45).
///
/// Before this existed, `run` and `run_mini` did `enable_raw_mode()?;
/// execute!(..., EnterAlternateScreen)?; ...; Terminal::new(backend)?; ...;
/// poll_loop(...)?;` with the restore written out by hand *after* all of those `?`s — so an
/// `Err` from `Terminal::new` or the poll loop skipped the restore block entirely, and the
/// user was dropped back into a shell still rendering the alternate buffer with no `reset`
/// sequence sent. `install_panic_hook` only covers *panics*; this covers `Err` returns, the
/// case it does not.
///
/// A `Drop` impl removes the class rather than the instance: every `?` between
/// `TerminalGuard::enter()` and the end of the enclosing function unwinds through it, so
/// there is nowhere left to add a third copy of the same hole.
struct TerminalGuard;

impl TerminalGuard {
    /// Enable raw mode and enter the alternate screen, returning a guard that restores both
    /// on drop.
    ///
    /// The guard is constructed right after `enable_raw_mode()`, BEFORE the
    /// `EnterAlternateScreen` write — not after, and not by hand-catching that write's error.
    /// `execute!` can write and flush part of the escape sequence and still return `Err` (a
    /// Copilot review on this PR caught the earlier version, which only called
    /// `disable_raw_mode()` on that path): if any of those bytes reached the terminal before
    /// the failure, skipping the guard here would leave a partially-entered alternate screen
    /// with raw mode off and nothing left in scope to send `LeaveAlternateScreen`. With the
    /// guard already alive, the `?` below drops it on the way out and its `Drop` sends the
    /// leave sequence regardless of how much of the enter sequence actually landed —
    /// harmless if none of it did, corrective if some of it did.
    fn enter() -> std::io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        let guard = TerminalGuard;
        let mut stdout = std::io::stdout();
        crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut out = std::io::stdout().lock();
        let _ = crossterm::execute!(out, crossterm::terminal::LeaveAlternateScreen);
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Install a panic hook that restores the terminal (leaves alternate screen,
/// disables raw mode) before delegating to the previous hook so the panic
/// message still reaches stderr in a non-raw terminal. `std::panic::take_hook`
/// returns `Box<dyn Fn(&PanicHookInfo) + Send + Sync>` directly (Rust 1.97
/// no longer wraps it in `Box<dyn Any>`), so we own the previous hook
/// wholesale inside our wrapper closure.
fn install_panic_hook() {
    use std::panic::{self, PanicHookInfo};
    let previous: Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync> = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let mut out = std::io::stdout().lock();
        let _ = crossterm::execute!(out, crossterm::terminal::LeaveAlternateScreen);
        previous(info);
    }));
}

/// Which screen `poll_loop` is currently rendering. Dashboard is the default
/// landing screen (petri/SPEC.md §3.2 frames it as the ambient monitor) — S6
/// wires a one-way `Enter`-on-a-row transition to the Browser; `Tab` to
/// switch back is S7's job (petri/SPEC.md §9), not implemented here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Dashboard,
    Browser,
}

/// The main poll loop: draw the current state once, then only redraw on
/// meaningful events (keyboard input, resize, mtime change). `q` breaks.
///
/// `last_good` and `prefs` are read by the caller (`run`) BEFORE the
/// alternate screen is entered, not here — see `run`'s Step 1.5 doc comment
/// for why a warning from either must never fire once the alt screen is
/// live.
fn poll_loop(
    state_path: &std::path::Path,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    mut last_good: Option<petridish_core::schema::Radar>,
    prefs: Prefs,
) -> std::io::Result<u8> {
    // Initial mtime snapshot. We don't draw on ticks where nothing has
    // changed — the initial draw below is unconditional so we always paint
    // something on startup, but subsequent mtime comparisons rely on this
    // baseline. Mutated on each iteration so the new value propagates forward.
    let mut last_mtime = std::fs::metadata(state_path)
        .ok()
        .and_then(|m| m.modified().ok());

    // The loaded preferences are kept for the lifetime of the loop and mutated
    // in place, rather than rebuilt from scratch at each save site. Rebuilding
    // was a real bug, not a style question: every `Prefs { .. }` literal had to
    // name the new `tools` field, and each one named it as an empty map — so
    // every Tab switch silently wiped the user's stored tool choices (ACT-8's
    // whole point being that it asks once). Mutate-and-save cannot drift that
    // way when the next field is added.
    let mut prefs = prefs;
    let (mut screen, mut browser_state) = match prefs.last_screen {
        LastScreen::Dashboard => (Screen::Dashboard, None),
        LastScreen::Browser => {
            let bstate = last_good.as_ref().map(crate::browser::BrowserState::new);
            (Screen::Browser, bstate)
        }
    };
    let mut dashboard_state: Option<crate::dashboard::DashboardState> = match last_good.as_ref() {
        Some(radar) => Some(crate::dashboard::DashboardState::with_collapsed(
            radar,
            prefs.collapsed,
        )),
        None => None,
    };

    // The SPACE-1 activity feed. Deliberately NOT a field on `DashboardState`: that struct
    // is rebuilt wholesale by `DashboardState::new` on every reload below, so a feed living
    // there would be wiped on every scan tick — slice 1's finding 3 in a new costume.
    let mut feed = match last_good.as_ref() {
        Some(radar) => crate::feed::FeedState::seeded(radar),
        None => crate::feed::FeedState::default(),
    };

    // The ACT-8 tool picker, `Some` while it is open. It takes every keystroke
    // while it is up — a modal that let keys leak through to the list behind
    // it would be worse than no modal.
    let mut picker: Option<crate::picker::PickerState> = None;
    // The ACT-2 `?` help popup. Unlike the picker it has no interaction beyond
    // dismissal: any key while it is open closes it. Checked before the
    // picker/quit/screen dispatch below so it can consume every keystroke
    // while open, the same reason the picker is checked first.
    let mut help_open = false;
    // A one-line message shown over the Browser: "no tool for that", "this
    // project has no remote". Cleared by the next keystroke, so it never
    // becomes stale chrome.
    let mut notice: Option<String> = None;
    // Which action the open picker is configuring, kept alongside it so the
    // chosen program can be launched immediately rather than only stored.
    let mut picker_action: Option<crate::tools::Action> = None;

    // Initial draw — unconditional so we always paint something on startup.
    render_current(
        terminal,
        &last_good,
        screen,
        &dashboard_state,
        &browser_state,
        &picker,
        help_open,
        &notice,
        &feed,
        &prefs,
    );

    loop {
        // crossterm's `poll` returns true when *any* event is queued (Key,
        // Resize, Mouse, FocusGained/Lost). We handle `q` to break; everything
        // else (in particular Resize) is absorbed and the next draw tick will
        // pick up the new terminal size.
        let event_ready =
            crossterm::event::poll(std::time::Duration::from_secs(1)).unwrap_or(false);
        if event_ready && let Ok(crossterm::event::Event::Key(key)) = crossterm::event::read() {
            // Any keystroke dismisses a transient notice, so it can never
            // linger as stale chrome over a screen it no longer describes.
            notice = None;

            let handled = if help_open {
                // Any key closes the popup and nothing else happens this
                // keystroke — including `q`, deliberately: accidentally
                // quitting out of a help screen would be a bad surprise.
                help_open = false;
                true
            } else if let Some(ref mut p) = picker {
                // The picker is modal: it consumes EVERY key while open,
                // including `q`. Letting `q` quit out from under an open
                // dialog would be a surprising way to lose the answer the
                // user was in the middle of giving — and `Esc` is right
                // there, advertised in the popup's own footer.
                match p.on_key(key.code) {
                    crate::picker::Outcome::Pending => {}
                    crate::picker::Outcome::Cancelled => {
                        picker = None;
                        picker_action = None;
                    }
                    crate::picker::Outcome::Chosen { program, persist } => {
                        let action = picker_action.take();
                        picker = None;
                        if let Some(action) = action {
                            // `persist` is ACT-11's verb. A one-off launch
                            // (`Enter` in re-pick mode) deliberately leaves
                            // the stored default alone — writing it here
                            // would cost the user the very default they
                            // pressed the shifted key to bypass.
                            if persist {
                                // Store first, then act. If the launch
                                // fails the user has still been asked once
                                // and only once (ACT-8).
                                prefs.tools.insert(action.id.to_string(), program.clone());
                                if let Err(e) = prefs::save(&prefs::default_prefs_path(), &prefs) {
                                    eprintln!("petri: persisting the tool choice failed: {e}");
                                }
                                // The focus panel names the resolved tool on its
                                // ACTIONS rung and memoises that lookup, because
                                // resolving probes the filesystem once per
                                // candidate and the panel redraws every poll
                                // tick. This is the one moment the memo can go
                                // stale: the answer just changed, and without
                                // this the panel would keep advertising the tool
                                // the user just replaced until petri restarts.
                                crate::focus::invalidate_tool_cache();
                            }
                            let project = current_selected_project(
                                screen,
                                &last_good,
                                &browser_state,
                                &dashboard_state,
                            );
                            notice = run_action(terminal, &action, &program, project);
                        }
                    }
                }
                true
            } else if key.code == crossterm::event::KeyCode::Char('q') {
                // `q` always quits, even in filter input mode.
                return Ok(0);
            } else if screen == Screen::Dashboard {
                // `Tab` switches Dashboard → Browser (petri/SPEC.md §5).
                if key.code == crossterm::event::KeyCode::Tab {
                    // Build browser state lazily on the first Tab switch,
                    // only if we have a valid radar (State read failures
                    // happen on first run when no state file exists yet).
                    let bstate = last_good.as_ref().map(crate::browser::BrowserState::new);
                    screen = Screen::Browser;
                    browser_state = bstate;
                    prefs.last_screen = LastScreen::Browser;
                    prefs.collapsed = dashboard_state
                        .as_ref()
                        .map(|d| d.collapsed)
                        .unwrap_or([false, false, true, true]);
                    if let Err(e) = prefs::save(&prefs::default_prefs_path(), &prefs) {
                        eprintln!("petri S7: persist Tab switch failed: {e}");
                    }
                    true
                } else {
                    match key.code {
                        crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                            if let Some(ref mut dstate) = dashboard_state {
                                dstate.move_selection(-1);
                            }
                            true
                        }
                        crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                            if let Some(ref mut dstate) = dashboard_state {
                                dstate.move_selection(1);
                            }
                            true
                        }
                        // `Space` is contextual now (issue #30,
                        // `PROPOSAL-focus-panel.md` §7): a header still toggles
                        // its section, a project row opens the focus popup, and
                        // an open popup closes. `press_space` owns the whole
                        // table — `toggle_selected` is untouched, since `Enter`
                        // on a header and `s6_dashboard.rs` both still want it.
                        crossterm::event::KeyCode::Char(' ') => {
                            if let (Some(dstate), Some(radar)) = (&mut dashboard_state, &last_good)
                            {
                                dstate.press_space(radar);
                            }
                            true
                        }
                        // `Enter`: on a header, toggle (same as Space); on a
                        // row, jump to the Browser with that project selected
                        // (petri/SPEC.md §5).
                        crossterm::event::KeyCode::Enter => {
                            if let (Some(dstate), Some(radar)) = (&mut dashboard_state, &last_good)
                            {
                                let current_row =
                                    dstate.selected.and_then(|i| dstate.visible.get(i)).copied();
                                match current_row {
                                    Some(crate::dashboard::DashRow::Header(_)) => {
                                        dstate.toggle_selected(radar);
                                    }
                                    Some(crate::dashboard::DashRow::Project(proj_idx)) => {
                                        // Persist Dashboard → Browser transition (same as Tab).
                                        prefs.last_screen = LastScreen::Browser;
                                        prefs.collapsed = dstate.collapsed;
                                        if let Err(e) =
                                            prefs::save(&prefs::default_prefs_path(), &prefs)
                                        {
                                            eprintln!(
                                                "petri S7: persist Enter→Browser failed: {e}"
                                            );
                                        }
                                        let mut bstate = crate::browser::BrowserState::new(radar);
                                        if let Some(pos) =
                                            bstate.visible.iter().position(|&i| i == proj_idx)
                                        {
                                            bstate.selected = Some(pos);
                                        }
                                        browser_state = Some(bstate);
                                        screen = Screen::Browser;
                                    }
                                    None => {}
                                }
                            }
                            true
                        }
                        // `Esc` closes the focus popup if one is open, and is
                        // otherwise the same no-op-that-redraws it has always
                        // been. `close_focus` reports whether it consumed the
                        // key so this stays a fall-through rather than a
                        // special case.
                        crossterm::event::KeyCode::Esc => {
                            if let Some(ref mut dstate) = dashboard_state {
                                dstate.close_focus();
                            }
                            true
                        }
                        // Action keys (issue #64). Last arm, same ordering
                        // rule as the Browser's identical arm below: every
                        // navigation binding above keeps priority, so an
                        // action can never steal `j`/`k`/`Space`/`Enter`.
                        // Fires whether or not the focus popup is open — the
                        // popup has no key handling of its own, it just
                        // renders whatever `dstate.selected` points at
                        // (`focus_target`'s doc comment), so the project this
                        // dispatches to is exactly the one the popup shows.
                        crossterm::event::KeyCode::Char(c) => {
                            let registry = crate::tools::registry();
                            let lower = registry.iter().find(|a| a.key == c).cloned();
                            let shifted = registry
                                .iter()
                                .find(|a| a.key.to_ascii_uppercase() == c && a.key != c)
                                .cloned();
                            let project = dashboard_state
                                .as_ref()
                                .zip(last_good.as_ref())
                                .and_then(|(d, r)| d.selected_project(r));
                            match (lower, shifted) {
                                (Some(action), _) => {
                                    notice = begin_action(
                                        terminal,
                                        &action,
                                        project,
                                        &prefs,
                                        &mut picker,
                                        &mut picker_action,
                                    );
                                    true
                                }
                                (None, Some(action)) => {
                                    notice = begin_repick(
                                        &action,
                                        project,
                                        &mut picker,
                                        &mut picker_action,
                                    );
                                    true
                                }
                                (None, None) => false,
                            }
                        }
                        _ => false,
                    }
                }
            } else if key.code == crossterm::event::KeyCode::Char('/') {
                // Enter filter input mode. The query starts empty and
                // subsequent character keys append to it. The flag lives on
                // `BrowserState` because `browser::render` needs it too — the
                // ACT-10 header chip draws differently while you are typing.
                if let Some(ref mut state) = browser_state {
                    state.filter_input = true;
                    state.filter_query = String::new();
                    if let Some(ref radar) = last_good {
                        state.apply_filter(radar, "");
                    }
                }
                true
            } else if browser_state.as_ref().is_some_and(|s| s.filter_input) {
                match key.code {
                    // Navigation arrows and j/k still move selection while
                    // in filter mode (the user may want to test moves without
                    // exiting the filter). Place before the generic Char(c)
                    // arm so they take priority.
                    crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                        if let Some(ref mut state) = browser_state {
                            state.move_selection(-1);
                        }
                        true
                    }
                    crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                        if let Some(ref mut state) = browser_state {
                            state.move_selection(1);
                        }
                        true
                    }
                    // Page/fast-jump/edge navigation, same as normal mode
                    // (see that match arm's comments) — none of these are
                    // printable characters that a filter query could want,
                    // so binding them here doesn't cost the user anything
                    // they could otherwise type.
                    crossterm::event::KeyCode::PageUp => {
                        if let Some(ref mut state) = browser_state {
                            let step = crossterm::terminal::size()
                                .map(|(w, h)| crate::browser::page_size(w, h) as i32)
                                .unwrap_or(BROWSER_FAST_JUMP);
                            state.move_selection(-step);
                        }
                        true
                    }
                    crossterm::event::KeyCode::PageDown => {
                        if let Some(ref mut state) = browser_state {
                            let step = crossterm::terminal::size()
                                .map(|(w, h)| crate::browser::page_size(w, h) as i32)
                                .unwrap_or(BROWSER_FAST_JUMP);
                            state.move_selection(step);
                        }
                        true
                    }
                    crossterm::event::KeyCode::Home => {
                        if let Some(ref mut state) = browser_state {
                            state.move_selection(i32::MIN);
                        }
                        true
                    }
                    crossterm::event::KeyCode::End => {
                        if let Some(ref mut state) = browser_state {
                            state.move_selection(i32::MAX);
                        }
                        true
                    }
                    // `Esc` closes the filter input mode *and* clears the
                    // query (petri/SPEC.md §5).
                    crossterm::event::KeyCode::Esc => {
                        if let Some(ref mut state) = browser_state {
                            state.filter_input = false;
                            state.filter_query = String::new();
                            if let Some(ref radar) = last_good {
                                state.apply_filter(radar, "");
                            }
                        }
                        true
                    }
                    // `Backspace` drops the last character of the query and
                    // re-filters. Not a "printable characters only" input:
                    // without this the only way out of a typo is `Esc` and
                    // retyping the whole query, which the ACT-10 chip made
                    // impossible to ignore once the query was on screen.
                    //
                    // `pop()` is char-wise, not byte-wise, so a multi-byte
                    // character deletes as one keypress rather than leaving
                    // a broken UTF-8 tail.
                    crossterm::event::KeyCode::Backspace => {
                        if let Some(ref mut state) = browser_state {
                            let mut q = std::mem::take(&mut state.filter_query);
                            q.pop();
                            if let Some(ref radar) = last_good {
                                state.apply_filter(radar, &q);
                            } else {
                                state.filter_query = q;
                            }
                            true
                        } else {
                            false
                        }
                    }
                    // `Enter` closes the filter input mode but keeps the
                    // query, so the filtered selection persists.
                    crossterm::event::KeyCode::Enter => {
                        if let Some(ref mut state) = browser_state {
                            state.filter_input = false;
                        }
                        true
                    }
                    // Character keys: append to the query (filter input
                    // only — we don't treat these as navigation when we're
                    // mid-filter). Non-printable / control keys fall
                    // through and are ignored in filter mode.
                    crossterm::event::KeyCode::Char(c) => {
                        if let Some(ref mut state) = browser_state {
                            let q = std::mem::take(&mut state.filter_query);
                            let new_q = format!("{q}{c}");
                            if let Some(ref radar) = last_good {
                                state.apply_filter(radar, &new_q);
                            }
                            true
                        } else {
                            false
                        }
                    }
                    _ => false,
                }
            } else {
                // `Tab` from the Browser switches back to the Dashboard
                // (petri/SPEC.md §5). Persistence is handled in the
                // Dashboard branch above, but here on the Browser side
                // it must also trigger a save (the Dashboard branch
                // doesn't fire when screen is Browser).
                if key.code == crossterm::event::KeyCode::Tab {
                    prefs.last_screen = LastScreen::Dashboard;
                    prefs.collapsed = dashboard_state
                        .as_ref()
                        .map(|d| d.collapsed)
                        .unwrap_or([false, false, true, true]);
                    if let Err(e) = prefs::save(&prefs::default_prefs_path(), &prefs) {
                        eprintln!("petri S7: persist Tab switch (Browser→Dashboard) failed: {e}");
                    }
                    screen = Screen::Dashboard;
                    true
                } else {
                    match key.code {
                        // Navigation in normal (non-filter) mode.
                        crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                            if let Some(ref mut state) = browser_state {
                                state.move_selection(-1);
                            }
                            true
                        }
                        crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                            if let Some(ref mut state) = browser_state {
                                state.move_selection(1);
                            }
                            true
                        }
                        // Fast jump: ~10 rows, a fixed hop independent of
                        // viewport size (PageUp/PageDown below is the
                        // screen-relative jump).
                        crossterm::event::KeyCode::Char('K') => {
                            if let Some(ref mut state) = browser_state {
                                state.move_selection(-BROWSER_FAST_JUMP);
                            }
                            true
                        }
                        crossterm::event::KeyCode::Char('J') => {
                            if let Some(ref mut state) = browser_state {
                                state.move_selection(BROWSER_FAST_JUMP);
                            }
                            true
                        }
                        // Page jump: exactly one screenful, matching the
                        // list's own real visible-row count (`browser::page_size`
                        // mirrors `browser::render`'s layout math). Falls back to
                        // the fixed fast-jump distance if the terminal size can't
                        // be read.
                        crossterm::event::KeyCode::PageUp => {
                            if let Some(ref mut state) = browser_state {
                                let step = crossterm::terminal::size()
                                    .map(|(w, h)| crate::browser::page_size(w, h) as i32)
                                    .unwrap_or(BROWSER_FAST_JUMP);
                                state.move_selection(-step);
                            }
                            true
                        }
                        crossterm::event::KeyCode::PageDown => {
                            if let Some(ref mut state) = browser_state {
                                let step = crossterm::terminal::size()
                                    .map(|(w, h)| crate::browser::page_size(w, h) as i32)
                                    .unwrap_or(BROWSER_FAST_JUMP);
                                state.move_selection(step);
                            }
                            true
                        }
                        // Jump straight to the first/last row.
                        crossterm::event::KeyCode::Home => {
                            if let Some(ref mut state) = browser_state {
                                state.move_selection(i32::MIN);
                            }
                            true
                        }
                        crossterm::event::KeyCode::End => {
                            if let Some(ref mut state) = browser_state {
                                state.move_selection(i32::MAX);
                            }
                            true
                        }
                        // `y` (IDEAS.md ACT-2): yank the selected project's path
                        // to the clipboard. Deliberately not a tools::registry()
                        // entry — see tools.rs's module doc / IDEAS.md's ACT-2
                        // table for why. `pbcopy` is spawned directly, piped
                        // stdin, no terminal hand-off (MECH-2/MECH-3 do not
                        // apply — nothing takes over the screen).
                        crossterm::event::KeyCode::Char('y') => {
                            notice = yank_selected_path(&last_good, &browser_state);
                            true
                        }
                        // `?` (IDEAS.md ACT-2): open the help popup.
                        crossterm::event::KeyCode::Char('?') => {
                            help_open = true;
                            true
                        }
                        // `Space` (issue #35): toggle the detail popup. Not
                        // modal like the help popup or picker — it lives on
                        // `BrowserState`, not a local flag, so navigation
                        // keeps working (and the popup's content keeps
                        // following the selection) while it's open;
                        // `browser::render` only actually draws it when the
                        // window is too narrow AND too short for either
                        // inline placement, so this is a harmless no-op
                        // otherwise.
                        crossterm::event::KeyCode::Char(' ') => {
                            if let Some(ref mut state) = browser_state {
                                state.detail_popup_open = !state.detail_popup_open;
                            }
                            true
                        }
                        // `Esc` in normal mode: closes the detail popup if
                        // one is open (issue #35); otherwise a no-op (only
                        // meaningful to close the filter, handled above).
                        crossterm::event::KeyCode::Esc => {
                            if let Some(ref mut state) = browser_state {
                                state.detail_popup_open = false;
                            }
                            true
                        }
                        // Action keys (IDEAS.md `ACT-2`). Last arm, so every
                        // navigation binding above keeps priority over the
                        // registry — a future action must never be able to
                        // silently steal `j`/`k`/`J`/`K`.
                        //
                        // Note where this sits: inside the NORMAL-mode match,
                        // never the `in_filter_input` one above. If it were in
                        // both, typing `g` into the `/` filter would launch a
                        // git browser instead of filtering. The two branches
                        // being structurally separate is what makes that safe;
                        // `s8_pty_actions.rs` gates it regardless.
                        crossterm::event::KeyCode::Char(c) => {
                            let registry = crate::tools::registry();
                            // The lowercase key runs the action; the SHIFTED
                            // variant of the same key re-picks it (ACT-11).
                            // Derived from `action.key` rather than
                            // hard-coded, so a future registry entry gets
                            // its shifted key for free. Note this sits
                            // after the J/K ×10 navigation arms, which keep
                            // priority — an action must never be able to
                            // steal a movement key.
                            let lower = registry.iter().find(|a| a.key == c).cloned();
                            let shifted = registry
                                .iter()
                                .find(|a| a.key.to_ascii_uppercase() == c && a.key != c)
                                .cloned();
                            let project = selected_project(&last_good, &browser_state);
                            match (lower, shifted) {
                                (Some(action), _) => {
                                    notice = begin_action(
                                        terminal,
                                        &action,
                                        project,
                                        &prefs,
                                        &mut picker,
                                        &mut picker_action,
                                    );
                                    true
                                }
                                (None, Some(action)) => {
                                    notice = begin_repick(
                                        &action,
                                        project,
                                        &mut picker,
                                        &mut picker_action,
                                    );
                                    true
                                }
                                (None, None) => false,
                            }
                        }
                        _ => false,
                    }
                }
            };
            if handled {
                render_current(
                    terminal,
                    &last_good,
                    screen,
                    &dashboard_state,
                    &browser_state,
                    &picker,
                    help_open,
                    &notice,
                    &feed,
                    &prefs,
                );
            }
        }

        // Re-read and re-render only when the mtime changed (petri/SPEC.md
        // §4 "Auto-poll: stat the state file's mtime on a short timer and
        // re-read + re-render only when it changed.").
        let new_mtime = std::fs::metadata(state_path)
            .ok()
            .and_then(|m| m.modified().ok());

        let mtime_changed = match (&last_mtime, new_mtime) {
            (Some(prev), Some(now)) => *prev != now,
            _ => false,
        };

        if mtime_changed {
            match read_state_file(state_path) {
                Ok(r) => {
                    // The Dashboard's selection anchor has to be read here, against the
                    // OUTGOING radar, because `DashRow::Project` holds an index into
                    // `radar.projects` and `absorb_snapshot` is about to replace that list.
                    // Resolving the index afterwards would name whichever project happens to
                    // occupy that slot in the new scan — the exact silent cursor-drift the
                    // anchor exists to prevent.
                    let dash_anchor = match (&dashboard_state, &last_good) {
                        (Some(d), Some(previous)) => d.selection_anchor(previous),
                        _ => None,
                    };
                    // Feed first, by construction: `absorb_snapshot` owns both snapshots, so
                    // the previous one cannot be dropped before it has been diffed.
                    last_good = absorb_snapshot(&mut feed, last_good.take(), r);
                    // Re-derive browser state from the new Radar, preserving the
                    // current filter query. Selection follows the previously-
                    // selected project when it survives, else resets to first row
                    // (per spec §3.1 — `apply_filter` guarantees this). We take a
                    // snapshot of the filter query first so we don't hold two
                    // borrows on `browser_state` at once.
                    let query_snapshot: Option<String> =
                        browser_state.as_ref().map(|s| s.filter_query.clone());
                    if let (Some(radar), Some(q)) = (&last_good, query_snapshot)
                        && let Some(ref mut state) = browser_state
                    {
                        state.apply_filter(radar, &q);
                    }
                    // Re-derive DashboardState too, regardless of which screen
                    // is currently active, so a reload while viewing the
                    // Browser still leaves a fresh Dashboard behind it.
                    //
                    // `refresh`, not `DashboardState::new`: the latter rebuilt
                    // with the hardcoded spec defaults, so every reload reopened
                    // sections the user had collapsed and threw the cursor back
                    // to the top. On a machine `swab` is actively scanning that
                    // is every few seconds, i.e. the screen rearranging itself
                    // under the user's hands with no input from them. The
                    // `dash_anchor` was captured above, against the outgoing
                    // radar, for the reason given there.
                    if let Some(ref radar) = last_good {
                        match dashboard_state {
                            Some(ref mut d) => d.refresh(radar, dash_anchor),
                            None => {
                                dashboard_state =
                                    Some(crate::dashboard::DashboardState::with_collapsed(
                                        radar,
                                        prefs.collapsed,
                                    ))
                            }
                        }
                    }
                }
                Err(e) => eprintln!("petri S5 mid-loop state read failed: {e}"),
            }
        }

        // Redraw only when something actually happened this tick: a crossterm
        // event (resize gets picked up here) or an mtime change. On quiet
        // ticks we skip draw so the output stream goes still — this keeps PTY
        // harnesses happy and the user's terminal clean when petri is idle.
        if event_ready || mtime_changed {
            render_current(
                terminal,
                &last_good,
                screen,
                &dashboard_state,
                &browser_state,
                &picker,
                help_open,
                &notice,
                &feed,
                &prefs,
            );
        }

        last_mtime = new_mtime;
    }
}

/// Fold a freshly-read snapshot into the activity feed and hand back the new `last_good`.
///
/// **This function exists to make the ordering unrepresentable rather than merely tested.**
/// The bug it forecloses is a one-liner: `last_good = Some(r)` destroys the previous
/// snapshot, and `FeedState::ingest` needs it — so a reload that assigns first silently
/// produces a feed that never grows a row, on a code path no unit test naturally covers.
/// Taking ownership of both halves means the caller *cannot* express that order. Same move
/// as slice 2's `run_action`, where removing a parameter beat adding a test.
///
/// - `last_good` is `None` (nothing parsed yet, first successful read): seed the feed from
///   `fresh` so a freshly-started `petri` has rows immediately.
/// - `last_good` is `Some(prev)`: ingest the `prev` -> `fresh` difference.
///
/// Returns `Some(fresh)`, which the caller stores as the new `last_good`.
pub fn absorb_snapshot(
    feed: &mut crate::feed::FeedState,
    last_good: Option<petridish_core::schema::Radar>,
    fresh: petridish_core::schema::Radar,
) -> Option<petridish_core::schema::Radar> {
    match last_good {
        // `prev` is consumed here and cannot outlive this arm, which is the point: there is
        // no way to write the replace-then-diff ordering that this function exists to
        // prevent.
        Some(prev) => feed.ingest(&prev, &fresh),
        None => *feed = crate::feed::FeedState::seeded(&fresh),
    }
    Some(fresh)
}

/// Helper: redraw `terminal` from the last good radar and whichever screen's
/// live state is currently active, with any read errors logged but not
/// propagated (mid-run failures degrade in place).
// See `render_section` in dashboard.rs: distinct render-state arguments, no
// natural grouping, so a params struct would be lint-driven noise.
#[allow(clippy::too_many_arguments)]
fn render_current(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    radar: &Option<petridish_core::schema::Radar>,
    screen: Screen,
    dashboard_state: &Option<crate::dashboard::DashboardState>,
    browser_state: &Option<crate::browser::BrowserState>,
    picker: &Option<crate::picker::PickerState>,
    help_open: bool,
    notice: &Option<String>,
    feed: &crate::feed::FeedState,
    prefs: &Prefs,
) {
    let Some(r) = radar else { return };
    match screen {
        Screen::Dashboard => {
            if let Some(s) = dashboard_state {
                let _ = terminal.draw(|frame| {
                    crate::dashboard::render(frame, r, s, feed);
                    // The focus popup, drawn last for MECH-1's reason (`Clear`
                    // only blanks what is already in the buffer). The target is
                    // re-derived from the cursor here, every frame, and never
                    // cached — that is what makes the popup follow `j`/`k` and
                    // what keeps it correct across a reload the scanner
                    // re-sorted (`SPEC.md` §4.3).
                    if s.focus_open {
                        let ctx = crate::focus::FocusCtx {
                            radar: r,
                            target: s.focus_target(r),
                            now: chrono::Utc::now(),
                            feed: Some(feed),
                            prefs,
                        };
                        crate::dashboard::render_focus_overlay(frame, frame.area(), &ctx);
                    }
                    // Same overlay ordering as the Browser, drawn on top of
                    // everything above including the focus popup: the picker
                    // (ACT-8/ACT-11) and the one-line notice (issue #64's
                    // "no error is shown" half — `begin_action`/`begin_repick`
                    // could already set these on the Dashboard, but nothing
                    // ever drew them). `render_notice` lives in `browser.rs`
                    // but draws a plain `Frame` + `&str`, no `BrowserState`
                    // involved, so it is exactly as reusable here.
                    //
                    // No `help_open` leg here: `?` isn't bound on the
                    // Dashboard, so that branch would have no caller.
                    if let Some(p) = picker {
                        crate::picker::render(frame, p);
                    } else if let Some(text) = notice {
                        crate::browser::render_notice(frame, text);
                    }
                });
            }
        }
        Screen::Browser => {
            if let Some(s) = browser_state {
                // Resolved per frame rather than once at startup: `nerd_font_installed`
                // memoises the filesystem probe, so this is a match on an enum after the
                // first call, and reading it here keeps the flag a function of `prefs`
                // rather than a second piece of startup state to keep in sync.
                let nerd = prefs.use_nerd_fonts(&crate::exec::nerd_font_installed);
                let _ = terminal.draw(|frame| {
                    crate::browser::render(frame, r, s, nerd);
                    // The overlay is drawn last, after the screen beneath it —
                    // `Clear` only blanks what is already in the buffer, so
                    // ordering is the whole mechanism (MECH-1).
                    if let Some(p) = picker {
                        crate::picker::render(frame, p);
                    } else if help_open {
                        crate::help::render(frame);
                    } else if let Some(text) = notice {
                        crate::browser::render_notice(frame, text);
                    }
                });
            }
        }
    }
}

/// The Browser's currently-selected project, or `None` when nothing is
/// selected (an empty filtered list is a representable state —
/// `browser::BrowserState::selected` is deliberately an `Option`).
fn selected_project<'a>(
    radar: &'a Option<petridish_core::schema::Radar>,
    browser_state: &Option<crate::browser::BrowserState>,
) -> Option<&'a petridish_core::schema::Project> {
    let radar = radar.as_ref()?;
    let state = browser_state.as_ref()?;
    let pos = state.selected?;
    let idx = *state.visible.get(pos)?;
    radar.projects.get(idx)
}

/// The current screen's selected project, whichever screen that is (issue
/// #64). `begin_action`/`begin_repick`/`run_action` used to take `radar` and
/// `browser_state` directly and resolve the project themselves, which meant
/// only the Browser screen — the one state shape they knew about — could ever
/// reach them. This is the one place that knows both screens' selection
/// models (`browser::BrowserState::selected`+`visible` vs.
/// `dashboard::DashboardState::selected_project`, which also covers the focus
/// popup — it renders whatever `selected` points at, so there is no separate
/// "popup selection" to resolve).
fn current_selected_project<'a>(
    screen: Screen,
    radar: &'a Option<petridish_core::schema::Radar>,
    browser_state: &Option<crate::browser::BrowserState>,
    dashboard_state: &Option<crate::dashboard::DashboardState>,
) -> Option<&'a petridish_core::schema::Project> {
    match screen {
        Screen::Browser => selected_project(radar, browser_state),
        Screen::Dashboard => dashboard_state
            .as_ref()
            .zip(radar.as_ref())
            .and_then(|(d, r)| d.selected_project(r)),
    }
}

/// Resolve an action against this machine and this project — the stored-tool
/// chain plus `tools::resolve`, shared by every call site that needs an
/// answer (`begin_action`'s Dashboard/Browser dispatch and `--mini`'s
/// Ready-only dispatch, issue #64) so the two can never disagree about which
/// tool a project resolves to.
fn resolve_action(
    action: &crate::tools::Action,
    project: &petridish_core::schema::Project,
    prefs: &Prefs,
) -> crate::tools::Resolution {
    let facts = crate::tools::Facts {
        path: &project.path,
        url: project.git.github_url.as_deref(),
        is_repo: project.git.is_repo,
    };
    // `ACT-4`'s resolution order for the editor: the stored answer first, then
    // `$VISUAL`, then `$EDITOR`, then the registry probe. Reading the environment here
    // rather than in `tools::resolve` is what keeps that function pure and hermetically
    // testable. Measured on a real machine: both variables are frequently unset while
    // `code` sits on PATH, so this chain often yields nothing at all and the probe does
    // the real work.
    //
    // Each source is filtered by *executability*, not merely by being set — the fix for a
    // bug review caught. Taking the first source that was merely present collapsed a
    // four-step order into one guess: with `$VISUAL` naming an editor no longer on this
    // machine, `resolve` would find it uninstalled and skip straight to probing, so a
    // perfectly good `$EDITOR` was never consulted. "Present" and "usable" are different
    // questions and only the second one orders this chain.
    let usable = |name: String| crate::exec::is_installed(&name).then_some(name);
    let stored = prefs
        .tools
        .get(action.id)
        .cloned()
        .and_then(usable)
        .or_else(|| {
            (action.id == "edit")
                .then(|| {
                    std::env::var("VISUAL")
                        .ok()
                        .and_then(usable)
                        .or_else(|| std::env::var("EDITOR").ok().and_then(usable))
                })
                .flatten()
        });
    crate::tools::resolve(action, &facts, stored.as_deref(), &|p| {
        crate::exec::is_installed_probe(p)
    })
}

/// Press an action key: resolve it against this machine and this project, then
/// either run it, open the picker, or explain why neither happened.
///
/// Returns the notice to display, if any. `Ok`-shaped outcomes return `None` —
/// a successful launch needs no commentary.
#[allow(clippy::too_many_arguments)]
fn begin_action(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    action: &crate::tools::Action,
    project: Option<&petridish_core::schema::Project>,
    prefs: &Prefs,
    picker: &mut Option<crate::picker::PickerState>,
    picker_action: &mut Option<crate::tools::Action>,
) -> Option<String> {
    let Some(project) = project else {
        return Some("nothing selected".to_string());
    };
    match resolve_action(action, project, prefs) {
        crate::tools::Resolution::Ready(launch) => {
            launch_now(terminal, &launch, std::path::Path::new(&project.path))
        }
        crate::tools::Resolution::Ambiguous(installed) => {
            *picker = Some(crate::picker::PickerState::new(action, installed));
            *picker_action = Some(action.clone());
            None
        }
        crate::tools::Resolution::NoTool => Some(format!(
            "nothing installed that can {} — tried: {}",
            action.label,
            action
                .candidates
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        // `ACT-9`'s per-project axis, phrased in terms of the project rather
        // than the tooling: this is the half the user can see on the row in
        // front of them.
        // `ACT-9`'s per-project axis. The reason comes from the target rather
        // than being spelled here, so `o` on a project with no remote and `g` on
        // a directory that is not a repository each say their own true thing.
        crate::tools::Resolution::NoTarget => {
            Some(format!("{} {}", project.name, action.target.notice()))
        }
    }
}

/// Press a SHIFTED action key: open the re-pick popup (`ACT-11`).
///
/// Deliberately does not go through `tools::resolve`. That function collapses
/// an unambiguous choice to `Resolution::Ready` and throws the candidate list
/// away, so on the machines re-pick is *for* — one where a default already
/// resolves cleanly — there would be nothing left to list. `repick_candidates`
/// is the door that stays open.
fn begin_repick(
    action: &crate::tools::Action,
    project: Option<&petridish_core::schema::Project>,
    picker: &mut Option<crate::picker::PickerState>,
    picker_action: &mut Option<crate::tools::Action>,
) -> Option<String> {
    let Some(project) = project else {
        return Some("nothing selected".to_string());
    };
    let facts = crate::tools::Facts {
        path: &project.path,
        url: project.git.github_url.as_deref(),
        is_repo: project.git.is_repo,
    };
    match crate::tools::repick_candidates(action, &facts, &|p| crate::exec::is_installed_probe(p)) {
        // `ACT-9`'s per-project axis, phrased the same way `begin_action`
        // phrases it, so the two paths never disagree on screen.
        None => Some(format!("{} {}", project.name, action.target.notice())),
        // An empty list still opens the popup: `Other — specify path…` is
        // always a row, so a machine with nothing installed is still usable.
        Some(installed) => {
            *picker = Some(crate::picker::PickerState::repick(action, installed));
            *picker_action = Some(action.clone());
            None
        }
    }
}

/// Run the program the user just chose in the picker. Storing the answer
/// without acting on it would make the picker feel like a settings dialog
/// rather than the one keystroke it interrupted.
///
/// `program` is passed in explicitly rather than re-read from `prefs`, and
/// that is load-bearing for `ACT-11`. This function used to re-resolve through
/// `prefs.tools.get(action.id)`, which worked only because the caller always
/// wrote the answer to `prefs` first. A one-off launch deliberately does not
/// write it — so re-resolving would launch the user's OLD default, which is
/// exactly the behaviour the shifted key exists to escape.
///
/// The fix is structural rather than test-guarded: with no `prefs` parameter
/// in scope, this function *cannot* consult a stored answer even by accident,
/// which is a stronger guarantee than a test that a later refactor could
/// silently stop exercising. The event loop's half — persist only when the
/// picker says so — is covered by `s8_pty_repick.rs`.
/// The notice to show *instead of* launching, when this project cannot supply what the
/// action needs. `None` means go ahead.
///
/// **Rule 1 has to be re-checked on the launch path, not only where the picker opened.**
/// `tools::launch_for` maps a program name to a `Launch` and deliberately knows nothing
/// about targets, so without this the picker's choice bypasses the check entirely — and the
/// gap is reachable rather than theoretical: the poll loop keeps reloading `projects.json`
/// while the picker sits open as a modal, so the selected project can lose its `.git` (or
/// its remote) between the keypress that opened the picker and the choice that closes it.
/// `g` would then launch into a non-repo and git would exit at once, which is the exact
/// flash issue #38 exists to fix, reintroduced through the back door.
///
/// Public so an integration test can reach it: `lib.rs` has no unit-test module, and a
/// one-line guard nobody can call is a one-line guard nobody can prove.
pub fn launch_blocked_notice(
    action: &crate::tools::Action,
    project: &petridish_core::schema::Project,
) -> Option<String> {
    let facts = crate::tools::Facts {
        path: &project.path,
        url: project.git.github_url.as_deref(),
        is_repo: project.git.is_repo,
    };
    action
        .target
        .missing(&facts)
        .then(|| format!("{} {}", project.name, action.target.notice()))
}

fn run_action(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    action: &crate::tools::Action,
    program: &str,
    project: Option<&petridish_core::schema::Project>,
) -> Option<String> {
    let Some(project) = project else {
        return Some("nothing selected".to_string());
    };
    let facts = crate::tools::Facts {
        path: &project.path,
        url: project.git.github_url.as_deref(),
        is_repo: project.git.is_repo,
    };
    if let Some(notice) = launch_blocked_notice(action, project) {
        return Some(notice);
    }
    let launch = crate::tools::launch_for(action, &facts, program);
    launch_now(terminal, &launch, std::path::Path::new(&project.path))
}

/// Copy the selected project's path to the system clipboard via a piped
/// `pbcopy` child. macOS-only tool (matches the rest of petridish), so on the
/// Linux leg of the CI matrix this always degrades to a notice rather than
/// panicking or silently doing nothing — see invariant 5's "sensors degrade,
/// never abort" ethos, applied here even though this isn't a sensor.
fn yank_selected_path(
    radar: &Option<petridish_core::schema::Radar>,
    browser_state: &Option<crate::browser::BrowserState>,
) -> Option<String> {
    let Some(project) = selected_project(radar, browser_state) else {
        return Some("nothing selected".to_string());
    };
    let path = project.path.clone();
    let mut child = match std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Some(format!("could not copy to clipboard: {e}")),
    };
    let write_result = child.stdin.take().map(|mut stdin| {
        use std::io::Write;
        stdin.write_all(path.as_bytes())
    });
    if let Some(Err(e)) = write_result {
        let _ = child.wait();
        return Some(format!("could not copy to clipboard: {e}"));
    }
    match child.wait() {
        Ok(status) if status.success() => Some(format!("copied {path} to clipboard")),
        Ok(status) => Some(format!(
            "could not copy to clipboard: pbcopy exited {status}"
        )),
        Err(e) => Some(format!("could not copy to clipboard: {e}")),
    }
}

/// Run one resolved launch, turning every failure into a notice rather than an
/// error that would take the TUI down. A tool that is missing at launch time
/// (uninstalled since it was chosen) is a message, not a crash.
fn launch_now(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    launch: &crate::tools::Launch,
    cwd: &std::path::Path,
) -> Option<String> {
    match crate::exec::run(terminal, launch, cwd) {
        Ok(crate::exec::Outcome::Finished(_)) | Ok(crate::exec::Outcome::Detached) => None,
        Ok(crate::exec::Outcome::Failed(e)) => {
            Some(format!("could not run {}: {e}", launch.program))
        }
        Err(e) => Some(format!("terminal hand-off failed: {e}")),
    }
}
