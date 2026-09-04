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
};

const NO_REMOTE: Facts<'static> = Facts {
    path: "/Users/x/repos/thing",
    url: None,
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
    // carries seven of them, including `core.pager=less -R`, and mangling any
    // one would change what the user sees.
    let action = Action {
        id: "gitlog",
        key: 'g',
        label: "git history",
        target: Target::Path,
        candidates: vec![Candidate::new(
            "git",
            &["-c", "core.pager=less -R", "log", "--graph", "{path}"],
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
                "core.pager=less -R".to_string(),
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
fn browse_is_the_only_url_targeted_action() {
    for action in tools::registry() {
        let expected = if action.id == "browse" {
            Target::Url
        } else {
            Target::Path
        };
        assert_eq!(
            action.target, expected,
            "unexpected target on {:?}",
            action.id
        );
    }
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
