//! S8 acceptance gate, task 2 (petri/IDEAS.md `ACT-1`/`ACT-3`/`ACT-9`) —
//! **protected, authored by the orchestrator, not the delegate.**
//!
//! This file is the specification of `tools::resolve`. Every rule in that
//! function's doc comment has at least one test here, and the tests are
//! hermetic: they build their own fixture `Action`s and inject their own
//! "is it installed" closure, so they neither read `PATH` nor break when the
//! real registry gains a tool.
//!
//! The last section is different in kind — a handful of invariants over the
//! *real* `registry()`, which is orchestrator-authored data rather than
//! delegated logic. Those guard against a later edit to the registry
//! introducing a duplicate key or an incoherent target.

use petri::tools::{self, Action, Candidate, ExecMode, Facts, Launch, Resolution, Target};

/// `installed` closure over an explicit allow-list. Anything not named is
/// absent — the default must be "not installed" so a test can never
/// accidentally depend on the real machine.
fn only<'a>(programs: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
    move |p: &str| programs.contains(&p)
}

fn gitlog_fixture() -> Action {
    Action {
        id: "gitlog",
        key: 'g',
        label: "git history",
        target: Target::Path,
        candidates: vec![
            Candidate::new("serie", &[], ExecMode::Terminal),
            Candidate::new("lazygit", &["-p", "{path}"], ExecMode::Terminal),
            Candidate::new("git", &["log", "--graph"], ExecMode::Terminal).as_fallback(),
        ],
    }
}

fn browse_fixture() -> Action {
    Action {
        id: "browse",
        key: 'o',
        label: "open remote",
        target: Target::Url,
        candidates: vec![Candidate::new("open", &["{url}"], ExecMode::Background)],
    }
}

const PROJECT: Facts<'static> = Facts {
    path: "/Users/x/repos/thing",
    url: Some("https://github.com/x/thing"),
    is_repo: true,
};

const NO_REMOTE: Facts<'static> = Facts {
    path: "/Users/x/repos/thing",
    url: None,
    is_repo: true,
};

/// A project discovery admitted on a manifest file rather than a `.git` — issue #38's
/// case. Legitimate fleet member, nothing for `g` to show.
const NOT_A_REPO: Facts<'static> = Facts {
    path: "/Users/x/repos/notes",
    url: None,
    is_repo: false,
};

// ---------------------------------------------------------------- rule 1 --

#[test]
fn url_action_on_a_project_with_no_remote_is_no_target() {
    // ACT-9's second axis. The tooling is fine — `open` is installed — but
    // this project has nothing to open.
    let got = tools::resolve(&browse_fixture(), &NO_REMOTE, None, &only(&["open"]));
    assert_eq!(got, Resolution::NoTarget);
}

#[test]
fn no_target_beats_no_tool() {
    // Order matters and is asserted deliberately: when a project has no
    // remote AND nothing is installed, the answer is NoTarget, not NoTool.
    // The per-project fact is the one the user can see on the row in front of
    // them, so it is the one the dimmed affordance should be explained by.
    let got = tools::resolve(&browse_fixture(), &NO_REMOTE, None, &only(&[]));
    assert_eq!(got, Resolution::NoTarget);
}

#[test]
fn path_action_is_never_no_target() {
    // Every project has a path, so a Target::Path action can only ever fail
    // on tool availability.
    let got = tools::resolve(&gitlog_fixture(), &NO_REMOTE, None, &only(&[]));
    assert_eq!(got, Resolution::NoTool);
}

// ---------------------------------------------------------------- rule 2 --

#[test]
fn configured_and_installed_known_candidate_wins_outright() {
    // Two non-fallback candidates are installed, which would normally be
    // Ambiguous — but the user has already answered, so we do not re-ask.
    let got = tools::resolve(
        &gitlog_fixture(),
        &PROJECT,
        Some("lazygit"),
        &only(&["serie", "lazygit", "git"]),
    );
    assert_eq!(
        got,
        Resolution::Ready(Launch {
            program: "lazygit".to_string(),
            args: vec!["-p".to_string(), "/Users/x/repos/thing".to_string()],
            mode: ExecMode::Terminal,
        }),
        "a stored answer must use that candidate's own args and mode"
    );
}

#[test]
fn configured_program_the_registry_has_never_heard_of_still_runs() {
    // The picker's "Other — specify path…" answer. It gets a single target
    // argument and Terminal mode.
    let got = tools::resolve(
        &gitlog_fixture(),
        &PROJECT,
        Some("my-weird-git-tui"),
        &only(&["serie", "my-weird-git-tui"]),
    );
    assert_eq!(
        got,
        Resolution::Ready(Launch {
            program: "my-weird-git-tui".to_string(),
            args: vec!["/Users/x/repos/thing".to_string()],
            mode: ExecMode::Terminal,
        })
    );
}

#[test]
fn unknown_configured_program_for_a_url_action_gets_the_url_not_the_path() {
    // The single argument handed to an unknown program is the action's
    // target, so a Target::Url action passes the URL.
    let got = tools::resolve(
        &browse_fixture(),
        &PROJECT,
        Some("firefox"),
        &only(&["open", "firefox"]),
    );
    assert_eq!(
        got,
        Resolution::Ready(Launch {
            program: "firefox".to_string(),
            args: vec!["https://github.com/x/thing".to_string()],
            mode: ExecMode::Terminal,
        })
    );
}

#[test]
fn unknown_configured_program_defaults_to_terminal_mode_not_background() {
    // Asserted on its own because the asymmetry is the whole reason: guessing
    // Background for a terminal program corrupts the display; guessing
    // Terminal for a GUI program merely blocks petri until it is closed.
    let got = tools::resolve(
        &browse_fixture(),
        &PROJECT,
        Some("some-gui-browser"),
        &only(&["some-gui-browser"]),
    );
    match got {
        Resolution::Ready(launch) => assert_eq!(
            launch.mode,
            ExecMode::Terminal,
            "an unknown program must be assumed to want the terminal"
        ),
        other => panic!("expected Ready, got {other:?}"),
    }
}

#[test]
fn xdg_open_wins_on_a_machine_with_no_open() {
    // Issue #24: `open` doesn't exist on Linux, so on a synthetic PATH where
    // only `xdg-open` is present the *production* browse action (not a local
    // fixture, so a later edit to the real candidate list would break this
    // test rather than leaving it green) must still resolve unambiguously to
    // it rather than becoming NoTool.
    let reg = tools::registry();
    let browse = reg.iter().find(|a| a.id == "browse").expect("browse");
    let got = tools::resolve(browse, &PROJECT, None, &only(&["xdg-open"]));
    assert_eq!(
        got,
        Resolution::Ready(Launch {
            program: "xdg-open".to_string(),
            args: vec!["https://github.com/x/thing".to_string()],
            mode: ExecMode::Background,
        })
    );
}

#[test]
fn xdg_open_is_a_fallback_so_open_and_xdg_open_together_are_not_ambiguous() {
    // Copilot review on #72: `resolve` has no OS gate, so if `xdg-open` were an
    // ordinary candidate, a machine where both `open` and `xdg-open` resolve
    // `installed` would make browse Ambiguous instead of just picking `open`.
    // `xdg-open` is marked `as_fallback()` precisely so it never counts toward
    // that tally (mirrors gitlog's plain `git log --graph` fallback).
    let reg = tools::registry();
    let browse = reg.iter().find(|a| a.id == "browse").expect("browse");
    let got = tools::resolve(browse, &PROJECT, None, &only(&["open", "xdg-open"]));
    assert_eq!(
        got,
        Resolution::Ready(Launch {
            program: "open".to_string(),
            args: vec!["https://github.com/x/thing".to_string()],
            mode: ExecMode::Background,
        })
    );
}

// ---------------------------------------------------------------- rule 3 --

#[test]
fn configured_program_that_is_no_longer_installed_is_ignored() {
    // ACT-8: a removed tool must reopen the picker, not fail an exec against
    // a program that is gone. Here the stale answer falls through to a
    // genuinely ambiguous machine.
    let got = tools::resolve(
        &gitlog_fixture(),
        &PROJECT,
        Some("gitui"),
        &only(&["serie", "lazygit", "git"]),
    );
    match got {
        Resolution::Ambiguous(list) => {
            let names: Vec<&str> = list.iter().map(|c| c.program.as_str()).collect();
            assert_eq!(names, vec!["serie", "lazygit", "git"]);
        }
        other => panic!("expected Ambiguous after ignoring a stale choice, got {other:?}"),
    }
}

#[test]
fn stale_configured_program_falls_through_to_a_clean_single_winner() {
    let got = tools::resolve(
        &gitlog_fixture(),
        &PROJECT,
        Some("lazygit"),
        &only(&["serie", "git"]),
    );
    assert_eq!(
        got,
        Resolution::Ready(Launch {
            program: "serie".to_string(),
            args: vec![],
            mode: ExecMode::Terminal,
        }),
        "with the stale answer discarded, the one real candidate wins"
    );
}

// ---------------------------------------------------------------- rule 4 --

#[test]
fn two_real_candidates_and_no_stored_answer_is_ambiguous() {
    let got = tools::resolve(
        &gitlog_fixture(),
        &PROJECT,
        None,
        &only(&["serie", "lazygit"]),
    );
    match got {
        Resolution::Ambiguous(list) => {
            let names: Vec<&str> = list.iter().map(|c| c.program.as_str()).collect();
            assert_eq!(
                names,
                vec!["serie", "lazygit"],
                "registry order, installed only"
            );
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }
}

// ------------------------------------------------- candidate identity ------

/// Two candidates that share a `program` (the whole point of `as_app`: e.g.
/// several `open -a "<App>"` browser variants) but have distinct `id`s and
/// `probe`s. Only one of the two probes is "installed" in this fixture.
fn shared_program_fixture() -> Action {
    Action {
        id: "browse",
        key: 'o',
        label: "open remote",
        target: Target::Url,
        candidates: vec![
            Candidate::new("open", &["-a", "Chrome", "{url}"], ExecMode::Background)
                .as_app("chrome", "Chrome"),
            Candidate::new("open", &["-a", "Firefox", "{url}"], ExecMode::Background)
                .as_app("firefox", "Firefox"),
        ],
    }
}

#[test]
fn rule_4_installed_check_uses_probe_not_program_when_candidates_share_one() {
    // Both candidates run `program == "open"`, so an installed-check keyed on
    // `program` would see both as installed the instant `open` is anywhere on
    // PATH. Keyed on `probe` (each candidate's own "app:<Name>" key here),
    // only the one whose app is actually present resolves.
    let got = tools::resolve(
        &shared_program_fixture(),
        &PROJECT,
        None,
        &only(&["app:Firefox"]),
    );
    match got {
        Resolution::Ready(launch) => {
            assert_eq!(launch.args[1], "Firefox", "must resolve the installed one");
        }
        other => panic!("expected Ready(firefox), got {other:?}"),
    }
}

#[test]
fn rule_2_configured_lookup_uses_id_not_program_when_candidates_share_one() {
    // A stored preference names an `id`. Before candidate identity existed,
    // `launch_for` matched by `program`, so this would have resolved to
    // whichever `open`-based candidate came first in the list regardless of
    // which one the user actually stored. Matching by `id` picks the right
    // one even though a different, earlier candidate shares its `program`.
    // Both probes are installed here, deliberately: if `id`-matching ever
    // regressed back to matching by `program`, rule 2 would fail to find
    // "firefox" as a probe key, fall through to rule 4, and see *two*
    // installed non-fallback candidates (both `program == "open"`) --
    // Ambiguous, not Ready. That makes this test fail loudly on the
    // regression it exists to catch, rather than accidentally still passing
    // via rule 4 for the wrong reason.
    let got = tools::resolve(
        &shared_program_fixture(),
        &PROJECT,
        Some("firefox"),
        &only(&["app:Chrome", "app:Firefox"]),
    );
    match got {
        Resolution::Ready(launch) => {
            assert_eq!(
                launch.args[1], "Firefox",
                "configured id must win over an earlier same-program candidate"
            );
        }
        other => panic!("expected Ready(firefox), got {other:?}"),
    }
}

#[test]
fn rescan_is_bound_and_uses_swab_scan() {
    let reg = tools::registry();
    let rescan = reg
        .iter()
        .find(|a| a.id == "rescan")
        .expect("registry must carry a rescan action");
    assert_eq!(rescan.key, 's');
    assert_eq!(rescan.target, Target::Path);
    let got = tools::resolve(rescan, &PROJECT, None, &only(&["swab"]));
    match got {
        Resolution::Ready(launch) => {
            assert_eq!(launch.program, "swab");
            assert_eq!(launch.args, vec!["scan".to_string()]);
        }
        other => panic!("a machine with only `swab` installed must resolve, got {other:?}"),
    }
}

#[test]
fn a_lone_fallback_never_opens_the_picker() {
    // THE load-bearing rule. `git` is installed on every machine that can run
    // this repo. If fallbacks counted toward ambiguity, `gitlog` would be
    // Ambiguous for every user forever and the first-run picker would fire
    // for everyone — the exact opposite of ACT-8's "only ask when the choice
    // is genuinely ambiguous."
    let got = tools::resolve(&gitlog_fixture(), &PROJECT, None, &only(&["git"]));
    assert_eq!(
        got,
        Resolution::Ready(Launch {
            program: "git".to_string(),
            args: vec!["log".to_string(), "--graph".to_string()],
            mode: ExecMode::Terminal,
        }),
        "a fallback alone must run silently, never ask"
    );
}

#[test]
fn one_real_candidate_plus_a_fallback_runs_the_real_one_without_asking() {
    // The common case on a machine with exactly one git TUI installed: one
    // real candidate, plus `git` which is always there. Not ambiguous.
    let got = tools::resolve(
        &gitlog_fixture(),
        &PROJECT,
        None,
        &only(&["lazygit", "git"]),
    );
    assert_eq!(
        got,
        Resolution::Ready(Launch {
            program: "lazygit".to_string(),
            args: vec!["-p".to_string(), "/Users/x/repos/thing".to_string()],
            mode: ExecMode::Terminal,
        })
    );
}

#[test]
fn ambiguous_menu_still_lists_the_fallback() {
    // A fallback never *triggers* the menu, but it is a legitimate entry once
    // the menu is open: a user with lazygit installed may still prefer plain
    // `git log`. It sorts where the registry puts it, i.e. last.
    let got = tools::resolve(
        &gitlog_fixture(),
        &PROJECT,
        None,
        &only(&["serie", "lazygit", "git"]),
    );
    match got {
        Resolution::Ambiguous(list) => {
            let names: Vec<&str> = list.iter().map(|c| c.program.as_str()).collect();
            assert_eq!(names, vec!["serie", "lazygit", "git"]);
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }
}

#[test]
fn nothing_installed_at_all_is_no_tool() {
    let got = tools::resolve(&gitlog_fixture(), &PROJECT, None, &only(&[]));
    assert_eq!(got, Resolution::NoTool);
}

#[test]
fn a_single_real_candidate_runs_without_asking() {
    let got = tools::resolve(&browse_fixture(), &PROJECT, None, &only(&["open"]));
    assert_eq!(
        got,
        Resolution::Ready(Launch {
            program: "open".to_string(),
            args: vec!["https://github.com/x/thing".to_string()],
            mode: ExecMode::Background,
        }),
        "the sole candidate keeps its own Background mode"
    );
}

// ------------------------------------------------------- substitution ------

#[test]
fn only_placeholder_arguments_are_substituted() {
    // Literal arguments must survive untouched — `ACT-3`'s real git fallback
    // carries seven of them, including `core.pager=less -+F -+X -R`, and mangling any
    // one would change what the user sees.
    let action = Action {
        id: "gitlog",
        key: 'g',
        label: "git history",
        target: Target::Path,
        candidates: vec![Candidate::new(
            "git",
            &[
                "-c",
                "core.pager=less -+F -+X -R",
                "log",
                "--graph",
                "{path}",
            ],
            ExecMode::Terminal,
        )],
    };
    let got = tools::resolve(&action, &PROJECT, None, &only(&["git"]));
    assert_eq!(
        got,
        Resolution::Ready(Launch {
            program: "git".to_string(),
            args: vec![
                "-c".to_string(),
                "core.pager=less -+F -+X -R".to_string(),
                "log".to_string(),
                "--graph".to_string(),
                "/Users/x/repos/thing".to_string(),
            ],
            mode: ExecMode::Terminal,
        })
    );
}

#[test]
fn a_candidate_with_no_arguments_resolves_to_no_arguments() {
    // serie and tig take the repo from the working directory, which the
    // launcher sets. An empty template must stay empty rather than growing an
    // implicit path argument.
    let got = tools::resolve(&gitlog_fixture(), &PROJECT, None, &only(&["serie"]));
    match got {
        Resolution::Ready(launch) => assert!(
            launch.args.is_empty(),
            "expected no args, got {:?}",
            launch.args
        ),
        other => panic!("expected Ready, got {other:?}"),
    }
}

// --------------------------------------------------- real registry ---------

#[test]
fn registry_action_ids_are_unique() {
    // An id collision would make two actions share one `[tools]` preferences
    // key and silently overwrite each other's stored answer.
    let reg = tools::registry();
    let mut ids: Vec<&str> = reg.iter().map(|a| a.id).collect();
    ids.sort_unstable();
    let before = ids.len();
    ids.dedup();
    assert_eq!(
        before,
        ids.len(),
        "duplicate action id in registry: {ids:?}"
    );
}

#[test]
fn registry_candidate_ids_are_unique_per_action() {
    // A duplicate `id` within one action would make the stored preference
    // and `launch_for`'s lookup ambiguous between two candidates -- the exact
    // bug `Candidate::id` exists to prevent for candidates that share a
    // `program` (e.g. multiple `open -a "<App>"` browser variants).
    for action in tools::registry() {
        let mut ids: Vec<&str> = action.candidates.iter().map(|c| c.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(
            before,
            ids.len(),
            "action {:?} has a duplicate candidate id: {ids:?}",
            action.id
        );
    }
}

#[test]
fn registry_keys_are_unique() {
    let reg = tools::registry();
    let mut keys: Vec<char> = reg.iter().map(|a| a.key).collect();
    keys.sort_unstable();
    let before = keys.len();
    keys.dedup();
    assert_eq!(
        before,
        keys.len(),
        "two actions bound to the same key: {keys:?}"
    );
}

#[test]
fn every_registry_action_has_at_least_one_candidate() {
    for action in tools::registry() {
        assert!(
            !action.candidates.is_empty(),
            "action {:?} has no candidates and could never run",
            action.id
        );
    }
}

#[test]
fn registry_has_at_most_one_fallback_per_action() {
    // Two fallbacks would make "which last resort" an arbitrary,
    // untested-ordering decision.
    for action in tools::registry() {
        let n = action.candidates.iter().filter(|c| c.fallback).count();
        assert!(
            n <= 1,
            "action {:?} has {n} fallbacks, expected at most 1",
            action.id
        );
    }
}

#[test]
fn git_history_is_bound_and_always_resolvable_thanks_to_its_fallback() {
    // ACT-3's payoff, asserted as a property of the real registry: because
    // the chain ends in plain `git`, the `g` key can always be bound and
    // always honestly advertised in the footer (SPEC.md §3.1). Tool detection
    // decides *which* graph you get, never *whether* the key exists.
    let reg = tools::registry();
    let gitlog = reg
        .iter()
        .find(|a| a.id == "gitlog")
        .expect("registry must carry a gitlog action");
    assert_eq!(gitlog.key, 'g');
    let got = tools::resolve(gitlog, &PROJECT, None, &only(&["git"]));
    match got {
        Resolution::Ready(launch) => assert_eq!(launch.program, "git"),
        other => panic!("a machine with only git installed must still resolve, got {other:?}"),
    }
}

#[test]
fn git_history_fallback_disables_less_quit_if_one_screen() {
    // Regression for #39: git sets LESS=FRX when LESS is otherwise unset, so
    // merely pinning `core.pager=less -R` still inherits `-F` and exits
    // immediately for short logs. The command line must cancel it explicitly.
    let reg = tools::registry();
    let gitlog = reg
        .iter()
        .find(|a| a.id == "gitlog")
        .expect("registry must carry a gitlog action");
    let fallback = gitlog
        .candidates
        .iter()
        .find(|c| c.fallback)
        .expect("gitlog must have a fallback");
    assert_eq!(fallback.program, "git");
    assert!(
        fallback.args.iter().any(|arg| arg.contains("-+F")),
        "git fallback pager must disable less -F so short logs stay open: {:?}",
        fallback.args
    );
}

#[test]
fn git_history_fallback_disables_less_no_init_so_output_does_not_leak_into_the_shell() {
    // Regression for #62: git also sets the `X` flag in the same inherited
    // `LESS=FRX`, which tells less to skip the terminal's init/deinit strings
    // — the alternate-screen enter/leave. Left set, less draws straight into
    // the shell's normal buffer, so the log survives `q` and sticks around
    // after petri hands the terminal back. The command line must cancel it
    // explicitly, the same way it already cancels `F`.
    let reg = tools::registry();
    let gitlog = reg
        .iter()
        .find(|a| a.id == "gitlog")
        .expect("registry must carry a gitlog action");
    let fallback = gitlog
        .candidates
        .iter()
        .find(|c| c.fallback)
        .expect("gitlog must have a fallback");
    assert_eq!(fallback.program, "git");
    assert!(
        fallback.args.iter().any(|arg| arg.contains("-+X")),
        "git fallback pager must disable less -X so the log stays in its own alternate screen: {:?}",
        fallback.args
    );
}

#[test]
fn reveal_in_finder_is_bound_and_uses_open() {
    let reg = tools::registry();
    let reveal = reg
        .iter()
        .find(|a| a.id == "reveal")
        .expect("registry must carry a reveal action");
    assert_eq!(reveal.key, 'f');
    assert_eq!(reveal.target, Target::Path);
    let got = tools::resolve(reveal, &PROJECT, None, &only(&["open"]));
    match got {
        Resolution::Ready(launch) => {
            assert_eq!(launch.program, "open");
            assert_eq!(launch.args, vec![PROJECT.path.to_string()]);
        }
        other => panic!("a machine with only `open` installed must resolve, got {other:?}"),
    }
}

#[test]
fn each_action_declares_the_target_it_needs() {
    // Spelled as an explicit table rather than "browse is the odd one out", which is what
    // this test used to say: issue #38 gave `gitlog` a target of its own, and a rule phrased
    // as one exception silently mis-states the registry the moment there are two.
    for action in tools::registry() {
        let expected = match action.id {
            "browse" => Target::Url,
            "gitlog" => Target::GitRepo,
            _ => Target::Path,
        };
        assert_eq!(
            action.target, expected,
            "unexpected target on {:?}",
            action.id
        );
    }
}

// ---------------------------------------------------- issue #38: `g` ---------

#[test]
fn git_history_has_no_target_in_a_project_that_is_not_a_repo() {
    // #38: `g` on a non-repo used to resolve `Ready`, launch, and have git exit at once —
    // the screen flashed and came straight back. `SPEC.md` §5 forbids advertising a key
    // that does nothing, so this must be `NoTarget`, the same answer `o` gives a project
    // with no remote.
    let reg = tools::registry();
    let gitlog = reg.iter().find(|a| a.id == "gitlog").expect("gitlog");

    // Every git tool in the world installed makes no difference — this is a per-project
    // fact, and rule 1 is checked before any machine question.
    let got = tools::resolve(gitlog, &NOT_A_REPO, None, &|_| true);
    assert_eq!(got, Resolution::NoTarget);

    // ... and the same project still resolves the actions that only need a path.
    let reveal = reg.iter().find(|a| a.id == "reveal").expect("reveal");
    assert!(
        matches!(
            tools::resolve(reveal, &NOT_A_REPO, None, &only(&["open"])),
            Resolution::Ready(_)
        ),
        "a non-repo is a legitimate project, not a disabled one"
    );
}

#[test]
fn git_history_offers_no_repick_in_a_project_that_is_not_a_repo() {
    // `repick_candidates` carries its own copy of rule 1; if the two drift, `R` opens a
    // picker for an action that cannot run.
    let reg = tools::registry();
    let gitlog = reg.iter().find(|a| a.id == "gitlog").expect("gitlog");
    assert_eq!(
        tools::repick_candidates(gitlog, &NOT_A_REPO, &|_| true),
        None
    );
}

/// A project as the scanner would report it, with only the git facts this test cares about.
fn project_with(name: &str, is_repo: bool, url: Option<&str>) -> petridish_core::schema::Project {
    petridish_core::schema::Project {
        id: "id".to_string(),
        name: name.to_string(),
        path: format!("/repos/{name}"),
        category: "default".to_string(),
        parent_path: None,
        is_foreign: false,
        git: petridish_core::schema::GitState {
            is_repo,
            branch: None,
            is_dirty: false,
            uncommitted_files: 0,
            untracked_files: 0,
            last_commit_at: None,
            mine_last_commit_at: None,
            github_url: url.map(str::to_string),
            daily_commits: Vec::new(),
        },
        agent: petridish_core::schema::AgentState::idle_unknown(),
        last_activity_at: None,
        status_bucket: petridish_core::schema::StatusBucket::Cold,
        agent_activity: Vec::new(),
    }
}

#[test]
fn the_launch_path_re_checks_the_target_the_picker_did_not() {
    // `launch_for` knows nothing about targets, so the picker's choice would otherwise skip
    // rule 1 entirely. The gap is reachable: the poll loop reloads state while the picker is
    // open as a modal, so the selection can stop being a repo between the keypress that
    // opened it and the choice that closes it — and `g` would then launch into a non-repo,
    // reinstating the very flash #38 fixes.
    let reg = tools::registry();
    let gitlog = reg.iter().find(|a| a.id == "gitlog").expect("gitlog");
    let reveal = reg.iter().find(|a| a.id == "reveal").expect("reveal");
    let browse = reg.iter().find(|a| a.id == "browse").expect("browse");

    let not_a_repo = project_with("notes", false, None);
    let notice = petri::launch_blocked_notice(gitlog, &not_a_repo)
        .expect("g must be refused on a non-repo, not launched");
    assert!(
        notice.contains("notes") && notice.contains("not a git repository"),
        "the notice must name the project and the reason, got: {notice:?}"
    );

    // Same project, an action that only needs a path: not blocked.
    assert_eq!(
        petri::launch_blocked_notice(reveal, &not_a_repo),
        None,
        "a non-repo is a legitimate project, not a disabled one"
    );

    // And the URL axis goes through the same guard.
    assert!(petri::launch_blocked_notice(browse, &not_a_repo).is_some());
    let with_remote = project_with("thing", true, Some("https://github.com/x/thing"));
    assert_eq!(petri::launch_blocked_notice(browse, &with_remote), None);
    assert_eq!(petri::launch_blocked_notice(gitlog, &with_remote), None);
}

#[test]
fn resolve_action_only_launches_on_ready_the_seam_a_pty_frame_cannot_prove() {
    // Regression for a Copilot review on PR #66 (issue #64): the original `--mini` PTY test
    // for "an action with nowhere to resolve is a no-op" compared screens before and after —
    // but that comparison passes identically whether the new dispatch code ran and correctly
    // declined to launch, or never ran at all, since a declined `Resolution` produces no
    // repaint by design. That is not a real regression guard. `resolve_action` is the exact
    // seam `--mini`'s dispatch calls before deciding whether to hand off the terminal, so
    // asserting against ITS return value — deterministically, no PATH probing involved for
    // either case below — proves what the screen cannot.
    let reg = tools::registry();
    let browse = reg.iter().find(|a| a.id == "browse").expect("browse");
    let gitlog = reg.iter().find(|a| a.id == "gitlog").expect("gitlog");

    // No remote at all: `Target::Url` is missing, so `tools::resolve` short-circuits on
    // rule 1 before it ever probes an installed candidate (`petri/src/tools.rs`'s own doc
    // comment on rule 1) — deterministic on every machine, CI included.
    let no_remote = project_with("notes", true, None);
    assert_eq!(
        petri::resolve_action(browse, &no_remote, &petri::prefs::Prefs::default()),
        Resolution::NoTarget,
        "a project with no remote must resolve NoTarget, not silently do nothing"
    );

    // A stored answer of `true` — the same fixture `s14_pty_mini_actions.rs` seeds via
    // `petri.toml` for its real hand-off test — collapses straight to Ready without probing
    // any other candidate, since `true` is executable on every machine that can run this
    // suite (a POSIX builtin/coreutil).
    let repo = project_with("thing", true, None);
    let mut prefs = petri::prefs::Prefs::default();
    prefs.tools.insert("gitlog".to_string(), "true".to_string());
    match petri::resolve_action(gitlog, &repo, &prefs) {
        Resolution::Ready(launch) => assert_eq!(launch.program, "true"),
        other => panic!("a stored, installed answer must resolve Ready, got {other:?}"),
    }
}

#[test]
fn each_target_states_its_own_reason() {
    // The notice and the panel's dimmed entry both come from the target, so `o` and `g`
    // cannot end up telling the user the same wrong thing. Regression for the hardcoded
    // "has no remote" that used to serve both.
    assert_eq!(Target::Url.notice(), "has no remote");
    assert_eq!(Target::GitRepo.notice(), "is not a git repository");
    assert_ne!(Target::Url.short_reason(), Target::GitRepo.short_reason());

    assert!(Target::Url.missing(&NO_REMOTE));
    assert!(!Target::Url.missing(&PROJECT));
    assert!(Target::GitRepo.missing(&NOT_A_REPO));
    assert!(!Target::GitRepo.missing(&PROJECT));
    assert!(
        !Target::Path.missing(&NOT_A_REPO),
        "every project has a path"
    );
}

// ------------------------------------------------------- launch_for ----------

#[test]
fn launch_for_uses_a_known_candidate_args_and_mode() {
    // A stored answer that names a registry candidate keeps that candidate's
    // own args and mode.
    let got = tools::launch_for(&gitlog_fixture(), &PROJECT, "serie");
    assert_eq!(
        got,
        Launch {
            program: "serie".to_string(),
            args: vec![],
            mode: ExecMode::Terminal,
        },
        "a known candidate keeps its own args and mode"
    );

    // `serie` takes no arguments, so the assertion above cannot show that a
    // known candidate's `{path}` placeholder is still substituted. `lazygit`
    // is the case that can: same code path, but with a template to expand.
    let got = tools::launch_for(&gitlog_fixture(), &PROJECT, "lazygit");
    assert_eq!(
        got,
        Launch {
            program: "lazygit".to_string(),
            args: vec!["-p".to_string(), "/Users/x/repos/thing".to_string()],
            mode: ExecMode::Terminal,
        },
        "a known candidate's placeholders are substituted, not passed through"
    );
}

#[test]
fn launch_for_runs_an_unknown_program_with_a_single_target_argument() {
    // The picker's "Other — specify path…" answer: an unknown program gets a
    // single target argument and Terminal mode.
    let got = tools::launch_for(&gitlog_fixture(), &PROJECT, "my-weird-git-tui");
    assert_eq!(
        got,
        Launch {
            program: "my-weird-git-tui".to_string(),
            args: vec!["/Users/x/repos/thing".to_string()],
            mode: ExecMode::Terminal,
        }
    );
}

#[test]
fn launch_for_passes_the_url_to_an_unknown_program_on_a_url_action() {
    // The single argument is the action's target, so a Target::Url action
    // passes the URL.
    let got = tools::launch_for(&browse_fixture(), &PROJECT, "firefox");
    assert_eq!(
        got,
        Launch {
            program: "firefox".to_string(),
            args: vec!["https://github.com/x/thing".to_string()],
            mode: ExecMode::Terminal,
        }
    );
}

// --------------------------------------------------- repick_candidates -------

#[test]
fn repick_returns_a_lone_fallback_when_no_git_tui_is_installed() {
    // ACT-3's `git` fallback is on every machine that can run this repo, so a
    // machine with nothing but `git` still gets a re-pick popup listing it.
    let got = tools::repick_candidates(&gitlog_fixture(), &PROJECT, &only(&["git"]));
    assert_eq!(
        got,
        Some(vec![
            Candidate::new("git", &["log", "--graph"], ExecMode::Terminal,).as_fallback()
        ]),
        "the lone fallback is a valid re-pick entry"
    );
}

#[test]
fn repick_lists_all_four_git_tuis_plus_their_fallback() {
    // The real registry's git chain: serie, lazygit, gitui, tig, then the
    // always-available `git` fallback. All five installed → all five listed, in
    // registry order, no ambiguity gate, fallbacks included.
    let reg = tools::registry();
    let gitlog = reg
        .iter()
        .find(|a| a.id == "gitlog")
        .expect("registry must carry a gitlog action");
    let got = tools::repick_candidates(
        gitlog,
        &PROJECT,
        &only(&["serie", "lazygit", "gitui", "tig", "git"]),
    );
    assert_eq!(
        got,
        Some(vec![
            Candidate::new("serie", &[], ExecMode::Terminal),
            Candidate::new("lazygit", &["-p", "{path}"], ExecMode::Terminal),
            Candidate::new("gitui", &["-d", "{path}"], ExecMode::Terminal),
            Candidate::new("tig", &[], ExecMode::Terminal),
            Candidate::new(
                "git",
                &[
                    "-c",
                    "core.pager=less -+F -+X -R",
                    "log",
                    "--graph",
                    "--oneline",
                    "--decorate",
                    "--all",
                ],
                ExecMode::Terminal,
            )
            .as_fallback(),
        ])
    );
}

#[test]
fn repick_returns_an_empty_vec_when_nothing_is_installed() {
    // `Some(vec![])`, not `None`: the picker's "Other — specify path…" row is
    // always there, so edit on a machine with no editor still gets a popup.
    let reg = tools::registry();
    let edit = reg
        .iter()
        .find(|a| a.id == "edit")
        .expect("registry must carry an edit action");
    let got = tools::repick_candidates(edit, &PROJECT, &only(&[]));
    assert_eq!(got, Some(vec![]));
}

#[test]
fn repick_returns_none_for_a_url_action_with_no_remote() {
    // The same per-project guard as `resolve`'s rule 1: a URL action with no
    // remote has nothing to re-pick, so it returns None.
    let got = tools::repick_candidates(&browse_fixture(), &NO_REMOTE, &only(&["open"]));
    assert_eq!(got, None);
}

#[test]
fn repick_ignores_any_stored_choice_and_lists_every_installed_candidate() {
    // repick_candidates takes no `configured` argument by design, so a stored
    // answer can never steer it: with two real candidates installed — the exact
    // situation where `resolve` would be `Ambiguous` — it returns all of them.
    let got = tools::repick_candidates(&gitlog_fixture(), &PROJECT, &only(&["serie", "lazygit"]));
    assert_eq!(
        got,
        Some(vec![
            Candidate::new("serie", &[], ExecMode::Terminal),
            Candidate::new("lazygit", &["-p", "{path}"], ExecMode::Terminal),
        ]),
        "the whole installed set, no ambiguity gate, no stored choice"
    );
}
