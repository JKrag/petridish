//! Issue #64/#65: tool shortcuts against the real binary in `--mini`.
//!
//! Before issue #64, `mini_poll_loop`'s key handling was a bare `q`/`Esc` guard — every other
//! key, including every registry action key, was read and silently discarded. #64 wired up
//! the unambiguous `Resolution::Ready` case only; a resolution needing a picker or notice was
//! still a silent no-op, gated on a design decision (issue #65).
//!
//! #65's decision: no picker in `--mini` — its whole use case is a small pane a poweruser
//! already made their tool choice in — so `Resolution::Ambiguous` stays a no-op permanently.
//! `NoTool`/`NoTarget` gain a transient, centered notice instead of staying silent, since
//! those *are* worth explaining even in a poweruser's corner pane (e.g. `g` on a directory
//! that isn't a git repo).
//!
//! This file therefore covers three shapes at the PTY layer:
//! - `Ready` hands the terminal off for real (`an_unambiguous_action_hands_the_terminal_to_the_resolved_tool`).
//! - `NoTarget` paints a real, observable notice that then clears itself
//!   (`a_notarget_action_shows_a_transient_notice_then_clears_it`) — the two-way proof issue
//!   #65 asked for: appears, then times out.
//! - `Ambiguous` stays an explicit, asserted no-op
//!   (`an_ambiguous_action_stays_a_harmless_no_op_in_mini`), proven via a synthetic `PATH`
//!   with two fake "installed" editors so the resolution genuinely reaches `Ambiguous`
//!   rather than standing in for it. `tools::resolve`'s own classification into
//!   `Ready`/`Ambiguous`/`NoTool`/`NoTarget` is exhaustively covered by `s8_tools.rs`; what's
//!   proven here is only that `--mini`'s dispatch treats each shape the way issue #65 decided.
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
fn a_notarget_action_shows_a_transient_notice_then_clears_it() {
    // No remote at all, so `o` resolves to `Resolution::NoTarget` — issue #65's centered
    // notice, not a silent no-op, is the whole point of this test. The message text mirrors
    // `begin_action`'s `NoTarget` phrasing (`Target::notice()`'s "has no remote"), so a
    // divergence between the Dashboard/Browser wording and `--mini`'s would fail here.
    let home = scratch_home("notarget_notice");
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

    let notice_text = "mini-action-project has no remote";
    let with_notice = session.screen_until(
        90,
        24,
        Duration::from_secs(5),
        Duration::from_millis(300),
        8,
        |grid| grid.iter().any(|r| r.contains(notice_text)),
    );
    assert!(
        with_notice.iter().any(|r| r.contains(notice_text)),
        "a NoTarget action key in --mini must show a notice explaining why (issue #65), got:\n{}",
        with_notice.join("\n")
    );

    // `MINI_NOTICE_DURATION` (lib.rs) is 3s; poll well past it for the notice to clear
    // itself with no further keystroke.
    let cleared = session.screen_until(
        90,
        24,
        Duration::from_secs(1),
        Duration::from_millis(300),
        15,
        |grid| !grid.iter().any(|r| r.contains(notice_text)),
    );
    assert!(
        !cleared.iter().any(|r| r.contains(notice_text)),
        "the NoTarget notice must clear itself a few seconds after appearing (issue #65), \
         still present after the wait:\n{}",
        cleared.join("\n")
    );

    session.writer.write_all(b"q").expect("write q");
    session.writer.flush().expect("flush");
    let status = session.wait_with_timeout(Duration::from_secs(10));
    assert!(
        status.success(),
        "mini must still exit cleanly after a notice cycle, got exit status {status:?}"
    );
}

#[test]
fn a_notool_action_shows_a_transient_notice_then_clears_it() {
    // Copilot review on #77: the NoTarget test above doesn't cover NoTool, a distinct match
    // arm with its own message-building code in `mini_poll_loop`. `s` (rescan, `Target::Path`
    // — "never NoTarget" per `s8_tools.rs`) with `PATH` pointed at an empty directory forces
    // `swab` to resolve as not installed, landing reliably on `Resolution::NoTool` regardless
    // of what's actually on the machine running the test.
    let home = scratch_home("notool_notice");
    let state_path = state_file_pointing_at(&home, true, None);
    let empty_path = home.join("empty-path");
    std::fs::create_dir_all(&empty_path).expect("empty PATH dir must be creatable");

    let mut session = Session::spawn_with_args_and_env(
        &state_path,
        90,
        24,
        Some(&home),
        &["--mini", "mini-action-project"],
        &[("PATH", &empty_path.to_string_lossy())],
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

    session.writer.write_all(b"s").expect("write s");
    session.writer.flush().expect("flush");

    let notice_text = "nothing installed that can rescan now";
    let with_notice = session.screen_until(
        90,
        24,
        Duration::from_secs(5),
        Duration::from_millis(300),
        8,
        |grid| grid.iter().any(|r| r.contains(notice_text)),
    );
    assert!(
        with_notice.iter().any(|r| r.contains(notice_text)),
        "a NoTool action key in --mini must show a notice explaining why (issue #65), got:\n{}",
        with_notice.join("\n")
    );

    let cleared = session.screen_until(
        90,
        24,
        Duration::from_secs(1),
        Duration::from_millis(300),
        15,
        |grid| !grid.iter().any(|r| r.contains(notice_text)),
    );
    assert!(
        !cleared.iter().any(|r| r.contains(notice_text)),
        "the NoTool notice must clear itself a few seconds after appearing (issue #65), \
         still present after the wait:\n{}",
        cleared.join("\n")
    );

    session.writer.write_all(b"q").expect("write q");
    session.writer.flush().expect("flush");
    let status = session.wait_with_timeout(Duration::from_secs(10));
    assert!(
        status.success(),
        "mini must still exit cleanly after a NoTool notice cycle, got exit status {status:?}"
    );
}

#[test]
fn an_ambiguous_action_stays_a_harmless_no_op_in_mini() {
    // Issue #65's scope decision: --mini never gets a picker, so `Resolution::Ambiguous`
    // stays a no-op forever, not just until this issue ships. Proven for real rather than
    // assumed: a synthetic PATH makes two of the `edit` action's candidates ("nvim" and
    // "vim") genuinely "installed" per `is_installed_probe`, with no `[tools]` override in
    // prefs to collapse the choice — the same setup shape `s8_tools.rs` uses to reach
    // `Resolution::Ambiguous` in the first place, run here through the real binary.
    let home = scratch_home("ambiguous_noop");
    let state_path = state_file_pointing_at(&home, true, None);

    let bin_dir = home.join("fakebin");
    std::fs::create_dir_all(&bin_dir).expect("fake bin dir must be creatable");
    for name in ["nvim", "vim"] {
        let stub = bin_dir.join(name);
        std::fs::write(&stub, b"#!/bin/sh\nexit 0\n").expect("stub must be writable");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))
                .expect("stub must be made executable");
        }
    }
    let path_env = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut session = Session::spawn_with_args_and_env(
        &state_path,
        90,
        24,
        Some(&home),
        &["--mini", "mini-action-project"],
        &[("PATH", &path_env)],
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

    session.writer.write_all(b"e").expect("write e");
    session.writer.flush().expect("flush");

    // Copilot review on #77: exit status alone doesn't couple this test to the behavior it
    // claims to prove — a regression that launched one of the fake editors (they exit
    // immediately, same as `true`) or painted a notice would still exit 0. Two real checks
    // instead: no second alternate-screen entry (no hand-off happened at all, unlike the
    // `Ready` test above), and the pane still shows only the plain project view, no overlay.
    let no_handoff = !session.settle_until_raw(
        Duration::from_secs(2),
        Duration::from_millis(400),
        3,
        |stream| Session::alt_screen_entries(stream) >= 2,
    );
    assert!(
        no_handoff,
        "an Ambiguous action key must never hand the terminal off (issue #65 keeps it a \
         no-op) — got a second alternate-screen entry as if a candidate launched"
    );

    let after = session.screen_until(
        90,
        24,
        Duration::from_secs(2),
        Duration::from_millis(300),
        5,
        |grid| grid.iter().any(|r| r.contains("mini-action-project")),
    );
    assert!(
        after.iter().any(|r| r.contains("mini-action-project"))
            && !after
                .iter()
                .any(|r| r.contains("nothing installed") || r.contains("has no remote")),
        "an Ambiguous action key must leave the plain project view on screen, no notice \
         overlay (issue #65 keeps it a no-op), got:\n{}",
        after.join("\n")
    );

    session.writer.write_all(b"q").expect("write q");
    session.writer.flush().expect("flush");

    let status = session.wait_with_timeout(Duration::from_secs(10));
    assert!(
        status.success(),
        "an Ambiguous action key, followed by q, must still exit mini cleanly \
         (issue #65 keeps Ambiguous a no-op), got exit status {status:?}"
    );
}
