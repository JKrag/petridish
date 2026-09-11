//! Issue #64, layer 3: tool shortcuts against the real binary in `--mini`.
//!
//! Before this fix, `mini_poll_loop`'s key handling was a bare `q`/`Esc` guard — every other
//! key, including every registry action key, was read and silently discarded. The fix is
//! deliberately minimal, scoped down with the maintainer rather than inferred: only the
//! unambiguous `Resolution::Ready` case is wired up, since `--mini` has no picker and no
//! notice pane to show anything else in. Full parity (picker, notices) is issue #65.
//!
//! This file gates the `Ready` half at the PTY layer, where the launch hand-off is a real,
//! observable event. **It does not attempt the same for the non-`Ready` outcomes** — a
//! Copilot review on this PR caught an earlier version that claimed to, and the claim didn't
//! hold up: `dispatch_mini_action`'s body is a single `if let Resolution::Ready(..) = ..`
//! match, so "does nothing" for `Ambiguous`, `NoTool`, and `NoTarget` is exactly as
//! unobservable on screen as the pre-fix bug it replaces — a PTY test comparing before/after
//! frames would pass identically whether the new dispatch code ran and correctly declined,
//! or never ran at all. That's the same "no picker/notice surface" gap issue #65 exists to
//! close, reached from a different angle. What *is* real and PTY-observable about that half
//! is liveness: the process must not hang or crash on a key with nowhere to resolve, which
//! `an_action_with_nowhere_to_resolve_does_not_hang_mini` checks by demanding a clean exit
//! immediately afterward, rather than a screen comparison. Which specific non-`Ready`
//! variant (`Ambiguous`/`NoTool`/`NoTarget`) that single key reaches doesn't change what's
//! being proven, since all three take the same "not `Ready`" branch; `tools::resolve`'s own
//! classification into those three is already exhaustively covered by `s8_tools.rs`.
//!
//! Mirrors `s8_pty_handoff.rs`'s technique for the launch half: `true` is pre-answered as
//! the `gitlog` tool via a seeded `petri.toml`, so the hand-off is real (suspend, run,
//! restore) but has no side effect on the machine running the tests.

mod pty_support;
use pty_support::Session;
use std::io::Write;
use std::time::Duration;

/// Same shape as `s8_pty_handoff.rs`'s fixture builder: a single project pointed at a
/// directory that actually exists, derived from the real fixture so it cannot drift out of
/// step with the schema `petri` parses.
fn state_file_pointing_at(
    dir: &std::path::Path,
    is_repo: bool,
    github_url: Option<&str>,
) -> std::path::PathBuf {
    let project_dir = dir.join("mini-action-project");
    std::fs::create_dir_all(&project_dir).expect("project dir must be creatable");

    let text = std::fs::read_to_string(pty_support::fixture_path("loaded.json"))
        .expect("the real fixture must be readable");
    let mut radar: serde_json::Value =
        serde_json::from_str(&text).expect("the real fixture must parse");

    let mut project = radar["projects"][0].clone();
    project["name"] = serde_json::json!("mini-action-project");
    project["path"] = serde_json::json!(project_dir.to_string_lossy());
    project["git"]["is_repo"] = serde_json::json!(is_repo);
    project["git"]["github_url"] = match github_url {
        Some(u) => serde_json::json!(u),
        None => serde_json::Value::Null,
    };
    radar["projects"] = serde_json::json!([project]);

    let path = dir.join("radar.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&radar).expect("serialize"),
    )
    .expect("state file must be writable");
    path
}

fn scratch_home(tag: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!(
        "petri_s14_pty_mini_{tag}_home_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".petridish")).expect("scratch home must be creatable");
    home
}

#[test]
fn an_unambiguous_action_hands_the_terminal_to_the_resolved_tool() {
    let home = scratch_home("ready");
    // Pre-answer `gitlog` with `true` (ExecMode::Terminal candidates all run inside the
    // hand-off, and `true` ignores its argv and exits 0 at once) so `resolve` collapses
    // straight to `Resolution::Ready` with no picker involved.
    std::fs::write(
        home.join(".petridish").join("petri.toml"),
        b"last_screen = \"browser\"\ncollapsed = [false, false, true, true]\n\n[tools]\ngitlog = \"true\"\n",
    )
    .expect("seed prefs must be writable");
    let state_path = state_file_pointing_at(&home, true, None);

    let mut session = Session::spawn_with_args(
        &state_path,
        90,
        24,
        Some(&home),
        &["--mini", "mini-action-project"],
    );

    let before = session.screen_until(
        90,
        24,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
        |grid| grid.iter().any(|r| r.contains("mini-action-project")),
    );
    assert!(
        before.iter().any(|r| r.contains("mini-action-project")),
        "precondition: the pane must show the pinned project, got:\n{}",
        before.join("\n")
    );

    session.writer.write_all(b"g").expect("write g");
    session.writer.flush().expect("flush");

    // Same ordering rule as `s8_pty_handoff.rs`: the mode transition is the only
    // unambiguous signal, since `true` repaints nothing and the pre-`g` frame already
    // satisfies every grid predicate.
    let restored = session.settle_until_raw(
        Duration::from_secs(10),
        Duration::from_millis(400),
        8,
        |stream| Session::alt_screen_entries(stream) >= 2,
    );
    assert!(
        restored,
        "an unambiguous action key in --mini must hand the terminal off and take it back \
         (issue #64), got no second alternate-screen entry"
    );

    let after = session.screen_until(
        90,
        24,
        Duration::from_secs(10),
        Duration::from_millis(400),
        6,
        |grid| grid.iter().any(|r| r.contains("mini-action-project")),
    );
    assert!(
        after.iter().any(|r| r.contains("mini-action-project")),
        "the pane must repaint after the hand-off, got:\n{}",
        after.join("\n")
    );

    session.writer.write_all(b"q").expect("write q");
    session.writer.flush().expect("flush");
    session.wait_with_timeout(Duration::from_secs(10));
}

#[test]
fn an_action_with_nowhere_to_resolve_does_not_hang_mini() {
    // No remote at all, so `o` resolves to `Resolution::NoTarget` — a stand-in for any
    // non-`Ready` outcome, per this file's module doc. There is no screen effect to wait for
    // by design, so the proof here is liveness, not a before/after frame comparison: `o`
    // immediately followed by `q` must still produce a clean exit. A hang or a panic that
    // corrupts the terminal (leaving raw mode set, or the child stuck) would fail this via
    // `wait_with_timeout`'s own panic-on-hang rather than a flaky screen diff.
    let home = scratch_home("noop");
    let state_path = state_file_pointing_at(&home, true, None);

    let mut session = Session::spawn_with_args(
        &state_path,
        90,
        24,
        Some(&home),
        &["--mini", "mini-action-project"],
    );

    let before = session.screen_until(
        90,
        24,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
        |grid| grid.iter().any(|r| r.contains("mini-action-project")),
    );
    assert!(
        before.iter().any(|r| r.contains("mini-action-project")),
        "precondition: the pane must show the pinned project, got:\n{}",
        before.join("\n")
    );

    session.writer.write_all(b"o").expect("write o");
    session.writer.flush().expect("flush");
    session.writer.write_all(b"q").expect("write q");
    session.writer.flush().expect("flush");

    let status = session.wait_with_timeout(Duration::from_secs(10));
    assert!(
        status.success(),
        "an action key with nowhere to resolve, followed by q, must still exit mini \
         cleanly (issue #64), got exit status {status:?}"
    );
}
