//! The external-tool registry (`petri/IDEAS.md` `ACT-1`, `ACT-3`, `ACT-9`).
//!
//! `petri` is a router, not a reimplementation of every tool it points at
//! (`IDEAS.md` `FRAME-2`): great TUIs already exist for git history, and
//! `petri`'s unique asset is knowing *which project needs you right now*. This
//! module is the data and the decision procedure behind that hand-off — which
//! programs can satisfy an action, which one to actually run on this machine,
//! and when to stop guessing and ask the user instead.
//!
//! **This module is pure.** It never reads `PATH`, never reads the
//! environment, and never spawns anything. Tool presence arrives as an
//! injected closure and the user's stored answer arrives as a parameter, so
//! every rule below is testable without touching the machine it runs on.
//! Actually launching the resolved program is a separate concern (`MECH-2` /
//! `MECH-3`), and lives elsewhere.

/// How a chosen program takes over the terminal.
///
/// The distinction is not cosmetic: it decides whether `petri` tears its own
/// TUI down first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    /// `MECH-2`. Suspend `petri` — leave the alternate screen, disable raw
    /// mode — hand the child the terminal wholesale, and wait for it to exit.
    /// For anything that draws its own full-screen interface: `serie`,
    /// `lazygit`, `vim`, a pager.
    Terminal,
    /// `MECH-3`. Spawn detached and return immediately. `petri` keeps the
    /// terminal and never waits. For anything that opens its own window:
    /// `code`, `open`.
    Background,
}

/// What an action needs *from the project* in order to have anything to act
/// on — `ACT-9`'s second availability axis.
///
/// Distinct from tool availability, which is per-machine. A project with no
/// remote leaves `o` with nothing to open even on a machine where every tool
/// is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Needs only the project's path, which every project has.
    Path,
    /// Needs the project's `github_url`; unavailable when that is `None`.
    Url,
}

/// One concrete program that can satisfy an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Program name as it would be found on `PATH`, e.g. `"lazygit"`.
    pub program: String,
    /// Argument template. Two placeholders are substituted at resolution
    /// time — `{path}` with the project's path and `{url}` with its remote
    /// URL. An argument containing neither is passed through verbatim.
    pub args: Vec<String>,
    pub mode: ExecMode,
    /// Identity used for the stored preference and `launch_for`'s registry
    /// lookup. Defaults to `program` in `Candidate::new` -- only diverges via
    /// `as_app`, for candidates that share a `program` with a sibling (e.g.
    /// multiple `open -a "<App>"` variants) and need a distinct identity.
    pub id: String,
    /// The key passed to the `installed` probe closure. Defaults to `program`
    /// in `Candidate::new`; `as_app` sets it to `"app:<Name>"` so the caller's
    /// probe can check `/Applications/<Name>.app` instead of a `PATH` lookup.
    pub probe: String,
    /// A last-resort entry, not a menu item.
    ///
    /// This exists for exactly one reason, and without it the whole design
    /// breaks: `ACT-3`'s git-history chain ends in plain `git log --graph`,
    /// and `git` is installed on every machine that can build this repo. If
    /// fallbacks counted toward ambiguity, `gitlog` would resolve to
    /// [`Resolution::Ambiguous`] on *every* machine forever, so the first-run
    /// picker would fire for everyone — the exact opposite of `ACT-8`'s "only
    /// ask when the choice is genuinely ambiguous."
    ///
    /// So a fallback never *causes* the picker to open. It is still offered
    /// inside the menu once something else has opened it, since a user with
    /// `lazygit` installed may legitimately still prefer plain `git log`.
    pub fallback: bool,
}

impl Candidate {
    pub fn new<S: Into<String>>(program: S, args: &[&str], mode: ExecMode) -> Self {
        let program = program.into();
        Candidate {
            id: program.clone(),
            probe: program.clone(),
            program,
            args: args.iter().map(|a| (*a).to_string()).collect(),
            mode,
            fallback: false,
        }
    }

    /// Mark this candidate a last resort. See [`Candidate::fallback`].
    pub fn as_fallback(mut self) -> Self {
        self.fallback = true;
        self
    }

    /// Marks this candidate as one of several sharing the same `program`
    /// (e.g. multiple `open -a "<App>"` variants) that need a distinct
    /// identity and a real, per-app installed check instead of the
    /// `PATH`-lookup every other candidate gets.
    pub fn as_app(mut self, id: &str, app_name: &str) -> Self {
        self.id = id.to_string();
        self.probe = format!("app:{app_name}");
        self
    }
}

/// One thing the user can do to a project, and everything that can do it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// Stable identifier, and the key under which the user's choice is stored
    /// in the preferences file's `[tools]` table. Never renamed lightly — a
    /// rename silently discards everyone's stored answer.
    pub id: &'static str,
    /// The key that triggers it in the Browser.
    pub key: char,
    /// Short human label, for the footer and the picker's title.
    pub label: &'static str,
    pub target: Target,
    /// Candidates in preference order. The first installed one wins when the
    /// choice is unambiguous; the order is also the order the picker lists
    /// them, so the best guess sits under the cursor and `Enter` is the whole
    /// interaction.
    pub candidates: Vec<Candidate>,
}

/// The facts about the currently-selected project that actions act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts<'a> {
    pub path: &'a str,
    pub url: Option<&'a str>,
}

/// A fully-resolved, ready-to-run invocation. Placeholders are already
/// substituted; the caller supplies the working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    pub mode: ExecMode,
}

/// The outcome of resolving one action against one machine and one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Run this.
    Ready(Launch),
    /// Two or more real (non-fallback) candidates are installed and the user
    /// has not chosen between them: open the `ACT-8` picker. Carries every
    /// installed candidate in registry order — fallbacks included, since they
    /// are legitimate menu entries even though they never trigger the menu.
    Ambiguous(Vec<Candidate>),
    /// Nothing that can do this is installed on this machine.
    NoTool,
    /// The tooling is fine; this project has nothing to act on (`ACT-9`).
    NoTarget,
}

/// Substitute `{path}` and `{url}` in one argument template.
///
/// `{url}` is only reachable for a [`Target::Url`] action, which cannot
/// resolve past [`Resolution::NoTarget`] without a URL — so an absent URL
/// substitutes as the empty string rather than being an error case the caller
/// has to handle twice.
fn substitute(arg: &str, facts: &Facts) -> String {
    arg.replace("{path}", facts.path)
        .replace("{url}", facts.url.unwrap_or(""))
}

/// The real registry: every action `petri` can perform, in key order.
///
/// Hand-written product decisions, deliberately not delegated — which tools,
/// which argv, which precedence, and `Terminal`-vs-`Background` per candidate
/// are judgement calls, not mechanical ones. `resolve` below is the mechanical
/// half.
pub fn registry() -> Vec<Action> {
    vec![
        Action {
            id: "browse",
            key: 'o',
            label: "open remote",
            target: Target::Url,
            // macOS only, matching the rest of petridish. Plain `open` hands
            // the URL to whatever the user's default browser is -- the
            // correct amount of opinion for petri to have about it, and it
            // stays first/unmodified as the always-available baseline. The
            // named-app entries below are `open -a "<App>"`, each given a
            // distinct id/probe via `as_app` since they all share program
            // "open" -- see `Candidate::as_app`'s doc comment for why that
            // matters. Only apps with direct evidence of being installed on
            // the machine this was written for are listed.
            candidates: vec![
                Candidate::new("open", &["{url}"], ExecMode::Background),
                Candidate::new("open", &["-a", "Safari", "{url}"], ExecMode::Background)
                    .as_app("safari", "Safari"),
                Candidate::new(
                    "open",
                    &["-a", "Google Chrome", "{url}"],
                    ExecMode::Background,
                )
                .as_app("chrome", "Google Chrome"),
                Candidate::new("open", &["-a", "Arc", "{url}"], ExecMode::Background)
                    .as_app("arc", "Arc"),
                Candidate::new(
                    "open",
                    &["-a", "Brave Browser", "{url}"],
                    ExecMode::Background,
                )
                .as_app("brave", "Brave Browser"),
                Candidate::new("open", &["-a", "Chromium", "{url}"], ExecMode::Background)
                    .as_app("chromium", "Chromium"),
                Candidate::new(
                    "open",
                    &["-a", "Microsoft Edge", "{url}"],
                    ExecMode::Background,
                )
                .as_app("edge", "Microsoft Edge"),
                Candidate::new("open", &["-a", "Vivaldi", "{url}"], ExecMode::Background)
                    .as_app("vivaldi", "Vivaldi"),
                Candidate::new("open", &["-a", "Opera", "{url}"], ExecMode::Background)
                    .as_app("opera", "Opera"),
            ],
        },
        Action {
            id: "edit",
            key: 'e',
            label: "open in editor",
            target: Target::Path,
            // GUI editors are Background — `code $folder` returns immediately
            // and must not take the terminal. Terminal editors are Terminal;
            // all of these open a directory natively (netrw, helix's picker,
            // dired), which is `ACT-4`'s folder-vs-file question answered.
            candidates: vec![
                Candidate::new("code", &["{path}"], ExecMode::Background),
                Candidate::new("cursor", &["{path}"], ExecMode::Background),
                Candidate::new("zed", &["{path}"], ExecMode::Background),
                Candidate::new("nvim", &["{path}"], ExecMode::Terminal),
                Candidate::new("hx", &["{path}"], ExecMode::Terminal),
                Candidate::new("vim", &["{path}"], ExecMode::Terminal),
                Candidate::new("emacs", &["{path}"], ExecMode::Terminal),
            ],
        },
        Action {
            id: "gitlog",
            key: 'g',
            label: "git history",
            target: Target::Path,
            candidates: vec![
                // serie and tig take the repo from the working directory,
                // which the launcher sets to the project path.
                Candidate::new("serie", &[], ExecMode::Terminal),
                Candidate::new("lazygit", &["-p", "{path}"], ExecMode::Terminal),
                Candidate::new("gitui", &["-d", "{path}"], ExecMode::Terminal),
                Candidate::new("tig", &[], ExecMode::Terminal),
                // gitup and gitcomet are CLI-launchable GUI git clients,
                // registered as ExecMode::Background since petri's
                // spawn_detached path never blocks on the child regardless
                // of the child's own process model.
                Candidate::new("gitup", &[], ExecMode::Background),
                Candidate::new("gitcomet", &[], ExecMode::Background),
                // The always-available last resort (`ACT-3`). git pages
                // through less by itself when stdout is a tty, so `q` returns
                // to petri exactly as it does from the dedicated TUIs — this
                // is a real entry, not a consolation prize.
                //
                // The pager is pinned rather than inherited: git's own default
                // is `less -F -X`, and `-F` makes less print-and-exit when the
                // output fits one screen, so a short history would flash past
                // instead of behaving like a TUI. Measured on a real machine,
                // not assumed.
                Candidate::new(
                    "git",
                    &[
                        "-c",
                        "core.pager=less -R",
                        "log",
                        "--graph",
                        "--oneline",
                        "--decorate",
                        "--all",
                    ],
                    ExecMode::Terminal,
                )
                .as_fallback(),
            ],
        },
        Action {
            id: "reveal",
            key: 'f',
            label: "reveal in Finder",
            target: Target::Path,
            // macOS only, same reasoning as `browse`'s `open {url}`: `open` hands the
            // path to Finder, which is the correct amount of opinion for petri to have.
            // ranger and nnn are TUI file managers -- ExecMode::Terminal, since
            // they draw their own full-screen interface in the same terminal,
            // the same reasoning as gitlog's serie/lazygit/tig entries.
            candidates: vec![
                Candidate::new("open", &["{path}"], ExecMode::Background),
                Candidate::new("ranger", &["{path}"], ExecMode::Terminal),
                Candidate::new("nnn", &["{path}"], ExecMode::Terminal),
            ],
        },
        Action {
            id: "rescan",
            key: 's',
            label: "rescan now",
            target: Target::Path,
            // `swab scan` otherwise only runs on the 60s launchd StartInterval;
            // petri already polls projects.json's mtime every second and
            // reloads on change (lib.rs's main loop), so firing the scan is
            // the whole job here — no reload logic needed on petri's side.
            candidates: vec![Candidate::new("swab", &["scan"], ExecMode::Background)],
        },
    ]
}

/// Decide what pressing `action`'s key should actually do, right now, on this
/// machine, for this project.
///
/// Pure by construction: `installed` is the only window onto the machine, and
/// `configured` is the only window onto stored state.
///
/// - `configured` is the user's stored answer for this action — the program
///   name from the preferences file's `[tools]` table. For `edit`, the caller
///   is also responsible for folding in `$VISUAL`/`$EDITOR` before calling
///   (`ACT-4`'s resolution order: stored choice, then `$VISUAL`, then
///   `$EDITOR`, then probe). This function deliberately does not read the
///   environment itself.
/// - `installed` answers "is this program on `PATH`" for one program name.
///
/// The rules, in the order they are applied — see `petri/tests/s8_tools.rs`,
/// which enumerates every one of them:
///
/// 1. A [`Target::Url`] action with no URL resolves [`Resolution::NoTarget`],
///    before anything else is considered. Target availability is per-project
///    and beats every per-machine question (`ACT-9`).
/// 2. A `configured` program that is installed wins. If it names a known
///    candidate, that candidate's args and mode are used. If it names
///    something the registry has never heard of — the picker's "Other —
///    specify path…" answer — it runs with a single target argument and
///    [`ExecMode::Terminal`].
///
///    `Terminal` is the safe assumption for an unknown program, and the
///    asymmetry is the reason: guessing `Background` for a terminal program
///    corrupts the display, while guessing `Terminal` for a GUI program merely
///    blocks `petri` until that window is closed. One is a bug, the other is
///    an inconvenience.
/// 3. A `configured` program that is *not* installed is ignored entirely and
///    resolution falls through as if nothing were configured — `ACT-8`'s
///    "re-ask when the stored choice disappears from `PATH`", rather than
///    failing an exec against a program that is gone.
/// 4. Otherwise, among installed candidates: two or more non-fallback ones
///    means [`Resolution::Ambiguous`]; exactly one non-fallback one runs; zero
///    non-fallback but at least one fallback runs the first fallback; nothing
///    installed at all is [`Resolution::NoTool`].
pub fn resolve(
    action: &Action,
    facts: &Facts,
    configured: Option<&str>,
    installed: &dyn Fn(&str) -> bool,
) -> Resolution {
    // Rule 1: a URL action with no remote has nothing to act on. This is a
    // per-project fact and beats every per-machine question, so it is checked
    // before tools and before the stored answer.
    if action.target == Target::Url && facts.url.is_none() {
        return Resolution::NoTarget;
    }

    // Rule 2: a stored answer that is still installed wins outright. The
    // program-name → [`Launch`] mapping lives in [`launch_for`] (rule 2's body,
    // extracted so a one-off launch — which has a name but nothing stored —
    // can be built in one place, without re-deriving the Terminal-is-the-safe-
    // guess reasoning here).
    //
    // Two sub-cases, since `configured` may or may not name a real registry
    // candidate: if it does, its own `probe` decides installedness (so an
    // `as_app` candidate is checked by app-bundle presence, not a bare PATH
    // lookup); if it doesn't -- the picker's "Other — specify path…" answer,
    // e.g. a hand-typed program never in the registry -- fall back to a
    // direct PATH/path check on the stored string itself, exactly as before
    // candidate identity existed. Either way `launch_for` builds the actual
    // `Launch` so the two paths (known candidate vs. free-typed program)
    // never have to agree on anything beyond "found something to launch".
    if let Some(id) = configured {
        match action.candidates.iter().find(|c| c.id == id) {
            Some(candidate) if installed(candidate.probe.as_str()) => {
                return Resolution::Ready(build_launch(candidate, facts));
            }
            None if installed(id) => {
                return Resolution::Ready(launch_for(action, facts, id));
            }
            _ => {}
        }
    }

    // Rule 3: a stored answer that is no longer installed (or no longer names
    // a real candidate that is still installed) is ignored entirely — fall
    // through as though nothing were configured (`ACT-8`).
    //
    // Rule 4: otherwise decide from what is actually installed on this machine.
    let installed_candidates: Vec<&Candidate> = action
        .candidates
        .iter()
        .filter(|c| installed(c.probe.as_str()))
        .collect();

    match installed_candidates.iter().filter(|c| !c.fallback).count() {
        0 => {
            // No non-fallback installed. A lone fallback still runs silently
            // (the load-bearing rule); nothing installed at all is `NoTool`.
            installed_candidates
                .iter()
                .find(|c| c.fallback)
                .map(|fallback| Resolution::Ready(build_launch(fallback, facts)))
                .unwrap_or(Resolution::NoTool)
        }
        1 => Resolution::Ready(build_launch(installed_candidates[0], facts)),
        _ => Resolution::Ambiguous(installed_candidates.into_iter().cloned().collect()),
    }
}

/// Build a [`Launch`] from one candidate: the program copied as-is, each arg
/// run through `substitute`, and the candidate's own mode.
fn build_launch(candidate: &Candidate, facts: &Facts) -> Launch {
    Launch {
        program: candidate.program.clone(),
        args: candidate
            .args
            .iter()
            .map(|a| substitute(a, facts))
            .collect(),
        mode: candidate.mode,
    }
}

/// The single argument handed to an unknown program in rule 2: the project's
/// path for a [`Target::Path`] action, its remote URL for a [`Target::Url`]
/// one. Rule 1 has already ruled out a URL action with no remote, so the URL
/// branch is always reachable here.
fn action_target<'a>(target: Target, facts: &'a Facts<'a>) -> &'a str {
    match target {
        Target::Url => facts.url.unwrap_or(""),
        Target::Path => facts.path,
    }
}

/// Build the [`Launch`] a stored program name means for this action and
/// project — rule 2's body, extracted from [`resolve`] for exactly this reason.
///
/// A one-off launch has a program name but nothing stored, so `lib.rs` needs
/// to turn a name into a `Launch` directly. That rule used to be re-derived at
/// `resolve`'s call site; carrying it here in one place keeps the
/// "an unknown program is assumed to want the terminal" reasoning from drifting
/// between the resolver and the launcher (`MECH-2`/`MECH-3`).
///
/// `program` is the caller's stored answer, already known to be installed. If
/// it names a registry candidate, that candidate's args and mode are used;
/// otherwise — the picker's "Other — specify path…" answer — it runs with a
/// single target argument and [`ExecMode::Terminal`]. `Terminal` is the safe
/// guess for an unknown program: assuming `Background` for a terminal program
/// corrupts the display, assuming `Terminal` for a GUI program merely blocks
/// `petri` until that window is closed. One is a bug, the other an
/// inconvenience.
pub fn launch_for(action: &Action, facts: &Facts, id: &str) -> Launch {
    if let Some(candidate) = action.candidates.iter().find(|c| c.id == id) {
        build_launch(candidate, facts)
    } else {
        Launch {
            program: id.to_string(),
            // Already a concrete path/URL, not a template — deliberately not
            // run through `substitute`, which would be a no-op here and would
            // wrongly suggest a user-typed program name can carry placeholders
            // of its own.
            args: vec![action_target(action.target, facts).to_string()],
            mode: ExecMode::Terminal,
        }
    }
}

/// Every installed candidate for this action — registry order, fallbacks
/// included — or `None` when the project has nothing to act on.
///
/// This deliberately skips both `resolve`'s ambiguity gate and its stored-choice
/// lookup. [`Resolution::Ambiguous`] only appears when the choice is *unresolved*;
/// re-pick's whole premise is that the choice is resolved and the user wants to
/// override it anyway, so routing `G` through `resolve` would make it a no-op on
/// exactly the machines it is for. The function takes no `configured` argument,
/// so a stored choice can never steer it: it returns the whole installed set
/// (even an empty one) whenever the project has a target.
///
/// An empty `Vec` is a valid answer, not an error — a machine with nothing but
/// the picker's "Other — specify path…" row still gets a popup.
pub fn repick_candidates(
    action: &Action,
    facts: &Facts,
    installed: &dyn Fn(&str) -> bool,
) -> Option<Vec<Candidate>> {
    // Same per-project guard as `resolve`'s rule 1: a URL action with no remote
    // has nothing to open, so there is no re-pick to offer. The caller turns
    // that `None` into the existing "has no remote" notice.
    if action.target == Target::Url && facts.url.is_none() {
        return None;
    }

    // Registry order, fallbacks included, and no ambiguity test.
    let installed: Vec<Candidate> = action
        .candidates
        .iter()
        .filter(|c| installed(c.probe.as_str()))
        .cloned()
        .collect();
    Some(installed)
}
