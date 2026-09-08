//! `SURF-7` mount #31 — `petri --mini`'s argv contract and project resolution
//! (`PLAN-focus-panel.md` §6, Phase C).
//!
//! **Protected. Authored by the planner, not the implementer** (`PLAN-focus-panel.md` §1
//! and §3): these tests are the spec for T7, they were written before any implementation
//! existed, and a round that changes or deletes one is an automatic revert regardless of
//! what the failing-test count does.
//!
//! `parse_args`, `resolve_mini` and `MiniError`'s `Display` are `unimplemented!()` in the
//! Phase C scaffold, so every test here FAILS (panics) rather than errors.
//!
//! Two things are pinned here, and they are pinned *because they are decisions*, not
//! because they are hard:
//!
//! 1. **The argv contract.** `petri`'s first positional argument is already the
//!    state-file path — a documented test hook the PTY suite depends on — and there is no
//!    `clap` to arbitrate (`SPEC.md` §10). So `petri --mini foo` is genuinely ambiguous,
//!    and the resolution (operand-immediately-following wins) is written down in
//!    `parse_args`'s doc comment and asserted below. The cases that look like typos —
//!    `--mini --version`, `--mini state.json` — are the whole point.
//! 2. **Resolution is by path or name, every tick, never by index.** `SPEC.md` §4.3
//!    records the live bug: the scanner re-sorts `radar.projects` on every scan, so a
//!    held index silently starts pointing at a different project. A `--mini` pane in a
//!    corner for days is the worst place for that, because there is no list on screen to
//!    make the swap visible. `resolves_the_same_project_after_the_scanner_re_sorts` is
//!    the test for it.

use petri::{CliArgs, MiniError, MiniTarget, parse_args, resolve_mini};
use petridish_core::schema::Radar;
use std::path::{Path, PathBuf};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join(name)
}

fn load(name: &str) -> Radar {
    let text = std::fs::read_to_string(fixture_path(name))
        .unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("fixture {name} failed to deserialize into Radar: {e}"))
}

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

fn parse(args: &[&str]) -> CliArgs {
    parse_args(&argv(args)).unwrap_or_else(|e| panic!("parse_args({args:?}) errored: {e}"))
}

fn idx_of(radar: &Radar, name: &str) -> usize {
    radar
        .projects
        .iter()
        .position(|p| p.name == name)
        .unwrap_or_else(|| panic!("no project named {name} in the fixture"))
}

fn path_of(radar: &Radar, name: &str) -> String {
    radar.projects[idx_of(radar, name)].path.clone()
}

// ---------------------------------------------------------------------------
// argv
// ---------------------------------------------------------------------------

#[test]
fn no_arguments_is_the_full_dashboard_on_the_default_state_path() {
    assert_eq!(parse(&[]), CliArgs::default());
}

#[test]
fn the_first_positional_is_still_the_state_path() {
    // The shipped test hook the PTY suite drives the binary through. Breaking this
    // breaks `s4_pty.rs`, `s6_pty.rs` and every other PTY test at once.
    let args = parse(&["/tmp/state.json"]);
    assert_eq!(args.state_path, Some(PathBuf::from("/tmp/state.json")));
    assert_eq!(args.mini, None);
    assert!(!args.version);
}

#[test]
fn version_flags_are_recognised() {
    for flag in ["--version", "-V"] {
        assert!(parse(&[flag]).version, "{flag} must set version");
    }
}

#[test]
fn version_wins_from_any_position() {
    // Today's `main.rs` only inspects argv[1]; the contract widens that to any position,
    // which is a superset of the shipped behaviour rather than a change to it.
    assert!(parse(&["/tmp/state.json", "--version"]).version);
    assert!(parse(&["--mini", "--version"]).version);
}

#[test]
fn bare_mini_targets_the_cwd() {
    let args = parse(&["--mini"]);
    assert_eq!(args.mini, Some(MiniTarget::Cwd));
    assert_eq!(args.state_path, None);
}

#[test]
fn mini_takes_the_argument_immediately_following_it() {
    let args = parse(&["--mini", "glacier-db"]);
    assert_eq!(args.mini, Some(MiniTarget::Pinned("glacier-db".into())));
    assert_eq!(args.state_path, None);
}

#[test]
fn minis_operand_outranks_the_state_path_positional() {
    // The ambiguity, decided: in `petri --mini state.json` the operand is a *pin*, not a
    // path. Anyone wanting both writes `petri --mini <name> state.json`.
    let args = parse(&["--mini", "state.json"]);
    assert_eq!(args.mini, Some(MiniTarget::Pinned("state.json".into())));
    assert_eq!(args.state_path, None);
}

#[test]
fn a_state_path_before_mini_still_parses_as_the_state_path() {
    let args = parse(&["/tmp/state.json", "--mini"]);
    assert_eq!(args.state_path, Some(PathBuf::from("/tmp/state.json")));
    assert_eq!(args.mini, Some(MiniTarget::Cwd));
}

#[test]
fn both_a_pin_and_a_state_path_can_be_given_in_that_order() {
    let args = parse(&["--mini", "glacier-db", "/tmp/state.json"]);
    assert_eq!(args.mini, Some(MiniTarget::Pinned("glacier-db".into())));
    assert_eq!(args.state_path, Some(PathBuf::from("/tmp/state.json")));
}

#[test]
fn minis_operand_never_starts_with_a_dash() {
    // A `-`-leading argument is a flag, not a target — so this is `--mini` with no
    // operand plus an unrecognised flag, which is an error, not a pane pinned to `-x`.
    let err = parse_args(&argv(&["--mini", "-x"])).expect_err("-x is not a target");
    assert!(
        err.contains("-x"),
        "the message must name the argument it rejected, got: {err}"
    );
}

#[test]
fn an_unknown_flag_is_an_error() {
    let err = parse_args(&argv(&["--focus"])).expect_err("--focus is not a petri flag");
    assert!(err.contains("--focus"), "got: {err}");
}

#[test]
fn a_second_positional_is_an_error() {
    // Silently ignoring an argument the user clearly meant something by is worse than
    // saying so.
    let err = parse_args(&argv(&["a.json", "b.json"])).expect_err("two state paths");
    assert!(err.contains("b.json"), "got: {err}");
}

#[test]
fn mini_twice_is_an_error() {
    let err = parse_args(&argv(&["--mini", "a", "--mini", "b"])).expect_err("two --mini");
    assert!(err.contains("--mini"), "got: {err}");
}

// ---------------------------------------------------------------------------
// Resolution — the path walk
// ---------------------------------------------------------------------------

#[test]
fn the_cwd_at_a_project_root_resolves_to_that_project() {
    let radar = load("normal.json");
    let want = idx_of(&radar, "ember-core");
    let cwd = PathBuf::from(path_of(&radar, "ember-core"));

    assert_eq!(resolve_mini(&radar, &MiniTarget::Cwd, &cwd), Ok(want));
}

#[test]
fn a_subdirectory_resolves_to_the_project_above_it() {
    // The "walk up to the git toplevel first" nicety from `PROPOSAL-focus-panel.md` §8.1,
    // obtained for free by matching ancestors against `projects.json`'s own roots. No
    // `.git` probe, no second answer to "which project is this directory".
    let radar = load("normal.json");
    let want = idx_of(&radar, "ember-core");
    let cwd = PathBuf::from(path_of(&radar, "ember-core")).join("src/sensors");

    assert_eq!(resolve_mini(&radar, &MiniTarget::Cwd, &cwd), Ok(want));
}

#[test]
fn the_deepest_matching_project_wins() {
    // A project checked out inside another project's tree — a worktree under the parent,
    // or a vendored repo — must resolve to itself, not to the enclosing root. This is
    // why the walk goes deepest-first rather than taking the first `starts_with` hit.
    let mut radar = load("normal.json");
    let parent_path = path_of(&radar, "ember-core");
    let mut child = radar.projects[idx_of(&radar, "glacier-db")].clone();
    child.name = "nested-child".to_string();
    child.path = format!("{parent_path}/vendor/nested-child");
    child.id = "nested-child-id".to_string();
    radar.projects.push(child);
    let want = idx_of(&radar, "nested-child");

    let cwd = PathBuf::from(format!("{parent_path}/vendor/nested-child/src"));
    assert_eq!(resolve_mini(&radar, &MiniTarget::Cwd, &cwd), Ok(want));
}

#[test]
fn a_sibling_with_a_shared_prefix_is_not_a_match() {
    // String-prefix matching would make `/repos/ember-core-old` a subdirectory of
    // `/repos/ember-core`. Component-wise ancestry is the rule.
    let radar = load("normal.json");
    let cwd = PathBuf::from(format!("{}-old", path_of(&radar, "ember-core")));

    assert_eq!(
        resolve_mini(&radar, &MiniTarget::Cwd, &cwd),
        Err(MiniError::NotAProject(cwd.clone()))
    );
}

#[test]
fn a_cwd_outside_every_project_is_not_a_project() {
    let radar = load("normal.json");
    let cwd = PathBuf::from("/Users/jankrag/scratch/foo");

    assert_eq!(
        resolve_mini(&radar, &MiniTarget::Cwd, &cwd),
        Err(MiniError::NotAProject(cwd.clone()))
    );
}

// ---------------------------------------------------------------------------
// Resolution — a pinned target
// ---------------------------------------------------------------------------

#[test]
fn a_pinned_path_resolves_like_a_cwd() {
    let radar = load("normal.json");
    let want = idx_of(&radar, "horizon-gui");
    let pin = MiniTarget::Pinned(format!("{}/src", path_of(&radar, "horizon-gui")));

    // The cwd is somewhere else entirely: a pin must ignore it.
    let elsewhere = Path::new("/Users/jankrag/scratch");
    assert_eq!(resolve_mini(&radar, &pin, elsewhere), Ok(want));
}

#[test]
fn a_pinned_name_resolves_when_it_is_unique() {
    let radar = load("normal.json");
    let want = idx_of(&radar, "keystone-lib");
    let pin = MiniTarget::Pinned("keystone-lib".to_string());

    assert_eq!(
        resolve_mini(&radar, &pin, Path::new("/Users/jankrag/scratch")),
        Ok(want)
    );
}

#[test]
fn a_path_match_outranks_a_name_match() {
    // A project literally named after another project's path is absurd, but the ordering
    // still has to be stated: the path reading is tried first.
    let mut radar = load("normal.json");
    let target_path = path_of(&radar, "icebox");
    let want = idx_of(&radar, "icebox");
    let decoy = idx_of(&radar, "jewel-api");
    radar.projects[decoy].name = target_path.clone();

    assert_eq!(
        resolve_mini(
            &radar,
            &MiniTarget::Pinned(target_path),
            Path::new("/Users/jankrag/scratch")
        ),
        Ok(want)
    );
}

#[test]
fn an_unknown_pinned_name_is_unknown_not_not_a_project() {
    // The two errors are distinguished so the message can be specific about which of the
    // two readings the user meant. `nope` is not a path anyone typed as a path.
    let radar = load("normal.json");
    let pin = MiniTarget::Pinned("nope".to_string());

    assert_eq!(
        resolve_mini(&radar, &pin, Path::new("/Users/jankrag/scratch")),
        Err(MiniError::UnknownName("nope".to_string()))
    );
}

#[test]
fn an_ambiguous_pinned_name_is_an_error_carrying_sorted_candidates() {
    // Names are not unique: `SelectionAnchor`'s doc comment records a live fleet with
    // three projects called `smoke`. Picking one would look like it worked, which is the
    // worse failure — so this errors, and the candidate list is sorted so the message is
    // identical no matter how the scanner ordered the radar.
    let mut radar = load("normal.json");
    let mut twin = radar.projects[idx_of(&radar, "lantern")].clone();
    twin.path = "/Users/jankrag/repos/elsewhere/lantern".to_string();
    twin.id = "lantern-twin-id".to_string();
    let original_path = path_of(&radar, "lantern");
    radar.projects.push(twin);

    let mut want = vec![
        original_path,
        "/Users/jankrag/repos/elsewhere/lantern".to_string(),
    ];
    want.sort();

    assert_eq!(
        resolve_mini(
            &radar,
            &MiniTarget::Pinned("lantern".to_string()),
            Path::new("/Users/jankrag/scratch")
        ),
        Err(MiniError::AmbiguousName {
            name: "lantern".to_string(),
            paths: want
        })
    );
}

// ---------------------------------------------------------------------------
// The reason this is re-resolved every tick
// ---------------------------------------------------------------------------

#[test]
fn resolves_the_same_project_after_the_scanner_re_sorts() {
    // `SPEC.md` §4.3's live bug, in miniature: same radar, different order. Resolution
    // must follow the project, not the slot. A `--mini` pane holds a `MiniTarget` across
    // reloads precisely so this is true; if it held the `usize`, this test would fail by
    // showing a different project with no visible sign anything moved.
    let radar = load("normal.json");
    let pin = MiniTarget::Pinned("deltaflow".to_string());
    let cwd = Path::new("/Users/jankrag/scratch");
    let first = resolve_mini(&radar, &pin, cwd).expect("deltaflow resolves");
    let first_id = radar.projects[first].id.clone();

    let mut resorted = radar.clone();
    resorted.projects.reverse();
    let second = resolve_mini(&resorted, &pin, cwd).expect("deltaflow still resolves");

    assert_ne!(
        first, second,
        "precondition: reversing must actually move deltaflow's slot"
    );
    assert_eq!(
        resorted.projects[second].id, first_id,
        "the pin must follow the project across a re-sort, not stay on the index"
    );
}

#[test]
fn cwd_resolution_also_survives_a_re_sort() {
    let radar = load("normal.json");
    let cwd = PathBuf::from(path_of(&radar, "forest-net")).join("crates/inner");
    let first = resolve_mini(&radar, &MiniTarget::Cwd, &cwd).expect("forest-net resolves");
    let first_id = radar.projects[first].id.clone();

    let mut resorted = radar.clone();
    resorted.projects.reverse();
    let second = resolve_mini(&resorted, &MiniTarget::Cwd, &cwd).expect("still resolves");

    assert_eq!(resorted.projects[second].id, first_id);
}

// ---------------------------------------------------------------------------
// The messages (printed BEFORE the alternate screen is entered — `SPEC.md` §4.4)
// ---------------------------------------------------------------------------

#[test]
fn the_not_a_project_message_names_the_path_and_both_ways_out() {
    // `PROPOSAL-focus-panel.md` §8.3's shape: name the problem, then the two fixes.
    // Asserted by content rather than byte-for-byte — the path is `~`-abbreviated, which
    // depends on `$HOME`, and pinning the exact layout here would freeze wrapping the
    // renderer has no reason to promise.
    let msg = MiniError::NotAProject(PathBuf::from("/Users/jankrag/scratch/foo")).to_string();

    assert!(msg.contains("scratch/foo"), "must name the path: {msg}");
    assert!(
        msg.contains("not in projects.json"),
        "must name the problem: {msg}"
    );
    assert!(msg.contains("swab scan"), "must offer the rescan: {msg}");
    assert!(
        msg.contains("petri --mini <name>"),
        "must offer the pin: {msg}"
    );
}

#[test]
fn the_unknown_name_message_names_the_name() {
    let msg = MiniError::UnknownName("nope".to_string()).to_string();
    assert!(msg.contains("nope"), "{msg}");
    assert!(msg.contains("projects.json"), "{msg}");
}

#[test]
fn the_ambiguous_message_lists_every_candidate() {
    let msg = MiniError::AmbiguousName {
        name: "smoke".to_string(),
        paths: vec![
            "/Users/jankrag/repos/a/smoke".to_string(),
            "/Users/jankrag/repos/b/smoke".to_string(),
        ],
    }
    .to_string();

    assert!(msg.contains("smoke"), "{msg}");
    assert!(
        msg.contains("repos/a/smoke"),
        "must list candidate 1: {msg}"
    );
    assert!(
        msg.contains("repos/b/smoke"),
        "must list candidate 2: {msg}"
    );
}
