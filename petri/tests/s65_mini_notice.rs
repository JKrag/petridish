//! Issue #65's notice lifecycle (`mini_apply_resolution`/`mini_notice_expired`, `lib.rs`),
//! asserted deterministically instead of over a real wall-clock sleep or the real registry.
//!
//! A Copilot review on PR #77 flagged the PTY-layer proof of this (`s14_pty_mini_actions.rs`):
//! sleeping past `MINI_NOTICE_DURATION` in a real terminal session is exactly the
//! wall-clock-derived assertion `SPEC.md` §8 warns against ("inject a fixed clock"). The fix
//! there was a retry-budget bump, which reduces flake risk but doesn't remove the dependency
//! on real time. This file is the deterministic layer that dependency belongs in.
//!
//! Two seams make that possible:
//! - `mini_notice_expired` takes `now` as a parameter, so a synthetic "3 seconds later"
//!   instant (computed by `Duration` arithmetic, never slept) proves the boundary exactly.
//! - `mini_apply_resolution` takes an already-computed `Resolution` rather than resolving one
//!   itself, so these tests drive it with a synthetic fixture `Action`/`Resolution` instead
//!   of the real registry — which `resolution` a given key reaches through the real registry
//!   depends on what's installed on the machine running the test (and, for `edit`, on
//!   `$VISUAL`/`$EDITOR`), which is exactly the non-determinism this file exists to avoid.
//!   `s8_tools.rs` already exhaustively covers `tools::resolve`'s own classification into
//!   `Ready`/`Ambiguous`/`NoTool`/`NoTarget`; what's asserted here is only what `--mini` does
//!   with each one. The PTY suite (`s14_pty_mini_actions.rs`) keeps the real end-to-end
//!   proof — that `mini_poll_loop` is actually wired to these functions — since that's a
//!   claim only a real running binary can make.

use petri::mini_apply_resolution;
use petri::mini_notice_expired;
use petri::tools::{Action, Candidate, ExecMode, Launch, Resolution, Target};
use std::time::{Duration, Instant};

fn fixture_action() -> Action {
    Action {
        id: "fixture",
        key: 'x',
        label: "do the fixture thing",
        target: Target::GitRepo,
        candidates: vec![Candidate::new("fixture-tool", &[], ExecMode::Terminal)],
    }
}

fn project() -> petridish_core::schema::Project {
    petridish_core::schema::Project {
        id: "id".to_string(),
        name: "mini-notice-project".to_string(),
        path: "/repos/mini-notice-project".to_string(),
        category: "default".to_string(),
        parent_path: None,
        is_foreign: false,
        git: petridish_core::schema::GitState {
            is_repo: true,
            branch: None,
            is_dirty: false,
            uncommitted_files: 0,
            untracked_files: 0,
            last_commit_at: None,
            mine_last_commit_at: None,
            github_url: None,
            daily_commits: Vec::new(),
        },
        agent: petridish_core::schema::AgentState::idle_unknown(),
        last_activity_at: None,
        status_bucket: petridish_core::schema::StatusBucket::Cold,
        agent_activity: Vec::new(),
    }
}

fn fixture_launch() -> Launch {
    Launch {
        program: "fixture-tool".to_string(),
        args: Vec::new(),
        mode: ExecMode::Terminal,
    }
}

#[test]
fn ready_hands_off_the_launch_and_clears_a_stale_notice() {
    let action = fixture_action();
    let now = Instant::now();
    let mut notice = Some(("a stale notice from an earlier key".to_string(), now));

    let launch = mini_apply_resolution(
        Resolution::Ready(fixture_launch()),
        &action,
        &project(),
        &mut notice,
        now,
    );

    assert_eq!(launch, Some(fixture_launch()));
    assert!(
        notice.is_none(),
        "Ready must clear whatever notice preceded it (Copilot review on #77), got {notice:?}"
    );
}

#[test]
fn ambiguous_never_sets_or_clears_a_notice() {
    // Issue #65's scope call: --mini never gets a picker, so Ambiguous stays a permanent
    // no-op — it must neither explain itself nor silently eat a notice still on screen from
    // an earlier key.
    let action = fixture_action();
    let now = Instant::now();
    let mut with_stale = Some(("a stale notice from an earlier key".to_string(), now));
    let mut with_none: Option<(String, Instant)> = None;

    let ambiguous = || Resolution::Ambiguous(vec![]);

    let launch = mini_apply_resolution(ambiguous(), &action, &project(), &mut with_stale, now);
    assert!(launch.is_none());
    assert!(
        with_stale.is_some(),
        "Ambiguous must not clear a notice from an earlier key"
    );

    let launch = mini_apply_resolution(ambiguous(), &action, &project(), &mut with_none, now);
    assert!(launch.is_none());
    assert!(with_none.is_none(), "Ambiguous must never set a notice");
}

#[test]
fn no_tool_sets_a_notice_naming_the_action_and_every_candidate_tried() {
    let action = Action {
        candidates: vec![
            Candidate::new("first-tool", &[], ExecMode::Terminal),
            Candidate::new("second-tool", &[], ExecMode::Terminal),
        ],
        ..fixture_action()
    };
    let now = Instant::now();
    let mut notice = None;

    let launch = mini_apply_resolution(Resolution::NoTool, &action, &project(), &mut notice, now);

    assert!(launch.is_none(), "NoTool must never hand off a launch");
    let (text, started) = notice.expect("NoTool must set a notice");
    assert_eq!(
        text,
        "nothing installed that can do the fixture thing — tried: first-tool, second-tool"
    );
    assert_eq!(
        started, now,
        "the notice must be timestamped with the `now` it was given"
    );
}

#[test]
fn no_target_sets_a_notice_naming_the_project_and_the_reason() {
    let action = fixture_action(); // Target::GitRepo -> "is not a git repository"
    let now = Instant::now();
    let mut notice = None;

    let launch = mini_apply_resolution(Resolution::NoTarget, &action, &project(), &mut notice, now);

    assert!(launch.is_none(), "NoTarget must never hand off a launch");
    let (text, started) = notice.expect("NoTarget must set a notice");
    assert_eq!(text, "mini-notice-project is not a git repository");
    assert_eq!(started, now);
}

#[test]
fn a_notice_is_not_expired_the_instant_it_is_set() {
    let notice = Some(("text".to_string(), Instant::now()));
    assert!(!mini_notice_expired(&notice, Instant::now()));
}

#[test]
fn a_notice_expires_exactly_at_the_duration_boundary() {
    // `MINI_NOTICE_DURATION` is 3s (lib.rs); computed via `Duration` arithmetic on a fixed
    // `started` instant, never a real sleep, so this is exact rather than "probably enough
    // time has passed."
    let started = Instant::now();
    let notice = Some(("text".to_string(), started));

    assert!(
        !mini_notice_expired(&notice, started + Duration::from_millis(2999)),
        "must not expire one millisecond early"
    );
    assert!(
        mini_notice_expired(&notice, started + Duration::from_secs(3)),
        "must expire exactly at the boundary"
    );
    assert!(mini_notice_expired(
        &notice,
        started + Duration::from_secs(30)
    ));
}

#[test]
fn no_notice_is_never_expired() {
    assert!(!mini_notice_expired(
        &None,
        Instant::now() + Duration::from_secs(999)
    ));
}
