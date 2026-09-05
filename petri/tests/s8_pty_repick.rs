//! S8 acceptance gate, layer 3: the re-pick popup against the real binary
//! (petri/IDEAS.md `ACT-11`).
//!
//! **This file deliberately inverts `ACT-8`'s "tests must never see the
//! popup".** That rule protects the first-run picker, which appears only when
//! a choice is unresolved and would hang a PTY test that doesn't know to
//! answer it. Here the popup *is* the feature: `G` must open it even when the
//! choice resolves perfectly well, so the test opens it on purpose and always
//! answers it with `Esc`.
//!
//! Nothing here launches a real program. `Esc` is the only answer given, and
//! `Esc` is specified to launch nothing and change nothing — which is also the
//! strongest assertion available, since it lets the test check the preferences
//! file byte-for-byte. Note the baseline for that comparison is taken
//! immediately before `G`, not from the seed: the `Tab` that reaches the
//! Browser legitimately rewrites `last_screen`, and blaming re-pick for that
//! write would make the test lie about what it proves.
//!
//! The regression this guards is the one no unit test can see: `Enter` in
//! re-pick mode must not write `petri.toml`. A `PickerState` test can prove
//! the `persist: false` flag comes out of the state machine, but only the real
//! binary can prove `lib.rs` actually honours it before touching the file.

mod pty_support;
use pty_support::{Session, fixture_path};
use std::io::Write;
use std::time::Duration;

/// A user who already answered the picker for git history. This is precisely
/// the machine where `resolve` returns `Ready` and `Resolution::Ambiguous`
/// never appears — so if `G` were routed through `resolve`, it would have
/// nothing to show and this test would fail.
const SEEDED_PREFS: &[u8] =
    b"last_screen = \"dashboard\"\ncollapsed = [false, false, true, true]\n\n[tools]\ngitlog = \"serie\"\n";

fn seeded_home(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = std::env::temp_dir().join(format!("petri_s8_repick_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".petridish")).expect("scratch home must be creatable");
    let prefs_path = home.join(".petridish").join("petri.toml");
    std::fs::write(&prefs_path, SEEDED_PREFS).expect("seed prefs must be writable");
    (home, prefs_path)
}

fn settle(session: &mut Session) -> Vec<String> {
    session.screen_retry(
        90,
        40,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
    )
}

fn send(session: &mut Session, bytes: &[u8]) {
    session.writer.write_all(bytes).expect("write must succeed");
    session.writer.flush().expect("flush must succeed");
}

fn to_browser(home: &std::path::Path) -> Session {
    let mut session = Session::spawn_with_home(&fixture_path("loaded.json"), 90, 40, home);
    settle(&mut session);
    send(&mut session, b"\t");
    let screen = settle(&mut session);
    assert!(
        screen.iter().any(|r| r.contains("browser")),
        "expected to be on the Browser after Tab, got:\n{}",
        screen.join("\n")
    );
    session
}

#[test]
fn shift_g_opens_the_repick_popup_even_when_the_choice_already_resolves() {
    let (home, prefs_path) = seeded_home("opens");
    let mut session = to_browser(&home);

    // Snapshot AFTER the Tab that got us here: Tab legitimately rewrites
    // `last_screen`, so comparing against the seed would blame re-pick for
    // someone else's write. The baseline has to be "immediately before G".
    let before = std::fs::read(&prefs_path).expect("prefs must exist before G");

    send(&mut session, b"G");
    let screen = settle(&mut session);
    let body = screen.join("\n");

    assert!(
        body.contains("git history"),
        "G must open the popup titled with the action, got:\n{body}"
    );
    assert!(
        body.contains("this time"),
        "the popup must frame itself as a one-off, not a settings dialog:\n{body}"
    );
    // Both verbs, because both are live in this mode (SPEC.md §3.1).
    assert!(
        body.contains("run once"),
        "the popup must advertise the one-off verb:\n{body}"
    );
    assert!(
        body.contains("D set default"),
        "the popup must advertise the re-default verb:\n{body}"
    );

    send(&mut session, b"\x1b");
    let after = settle(&mut session);
    let after_body = after.join("\n");
    assert!(
        !after_body.contains("D set default"),
        "Esc must close the popup, got:\n{after_body}"
    );
    assert!(
        after_body.contains("browser"),
        "Esc must return to the Browser, not launch anything:\n{after_body}"
    );

    // Esc changes nothing — the strongest available assertion that no
    // accidental write happened on the way in or out.
    let after = std::fs::read(&prefs_path).expect("prefs must still exist");
    assert_eq!(
        after, before,
        "opening and cancelling the re-pick popup must not touch petri.toml"
    );
    assert!(
        String::from_utf8_lossy(&after).contains("gitlog = \"serie\""),
        "the stored default must survive the popup verbatim"
    );

    send(&mut session, b"q");
    session.wait_with_timeout(Duration::from_secs(10));
}

#[test]
fn shift_g_does_not_fire_while_the_filter_has_focus() {
    // The same trap `s8_pty_actions.rs` guards for the lowercase keys. `G` is
    // an ordinary printable character, so a binding placed in the wrong branch
    // would pop a modal over a user who was typing a project name.
    let (home, _prefs_path) = seeded_home("filter");
    let mut session = to_browser(&home);

    send(&mut session, b"/");
    settle(&mut session);

    send(&mut session, b"G");
    let screen = settle(&mut session);
    let body = screen.join("\n");

    assert!(
        !body.contains("D set default"),
        "typing G into the filter must not open the re-pick popup:\n{body}"
    );

    send(&mut session, b"q");
    session.wait_with_timeout(Duration::from_secs(10));
}

#[test]
fn a_shifted_key_on_an_action_with_no_target_reports_it_instead_of_popping() {
    // The footer now promises `o/O`, so `O` must behave. It exercises the OTHER
    // arm of `begin_repick`: `browse` is `Target::Url`, so on a project with no
    // `github_url` there is nothing to re-pick and the answer is `ACT-9`'s
    // per-project notice, not an empty popup. `G` on the same row still opens,
    // because `gitlog` is `Target::Path` and every project has one — asserting
    // both here is what shows the difference is the action's target rather than
    // the row.
    let (home, _prefs_path) = seeded_home("no_target");
    let mut session = to_browser(&home);

    // alpha-02 is the fixture's first project with `github_url: null`.
    send(&mut session, b"/alpha-02");
    settle(&mut session);
    send(&mut session, b"\r");
    settle(&mut session);

    send(&mut session, b"O");
    let screen = settle(&mut session);
    let body = screen.join("\n");
    assert!(
        body.contains("no remote"),
        "O on a project with no remote must explain itself, got:\n{body}"
    );
    assert!(
        !body.contains("D set default"),
        "there is nothing to re-pick, so no popup:\n{body}"
    );

    // Same row, Target::Path action: the popup does open.
    send(&mut session, b"G");
    let screen = settle(&mut session);
    let body = screen.join("\n");
    assert!(
        body.contains("D set default"),
        "G must still open on the same project — every project has a path:\n{body}"
    );

    send(&mut session, b"\x1b");
    settle(&mut session);
    send(&mut session, b"q");
    session.wait_with_timeout(Duration::from_secs(10));
}
