//! S8 regression gate: a screen switch must not destroy the user's stored tool
//! choices (petri/IDEAS.md `ACT-8`, petri/SPEC.md §6).
//!
//! This exists because of a real bug, not a hypothetical one. Adding `tools`
//! to `Prefs` forced every `Prefs { .. }` struct literal in `lib.rs` to name
//! the new field, and each of the three did so as an empty map — so every
//! `Tab` press wrote a preferences file with `[tools]` emptied. The entire
//! point of `ACT-8` is that it asks once; a picker whose answer is wiped by
//! the next screen switch would ask forever.
//!
//! No pure test could catch it: `prefs::save` was called correctly with
//! exactly the struct it was handed, and every unit test passed. Only the real
//! binary, driven through a real key press, writing a real file, shows it. So
//! this test asserts on the file on disk after the fact rather than on
//! anything rendered.

mod pty_support;
use pty_support::{Session, fixture_path};
use std::io::Write;
use std::time::Duration;

#[test]
fn switching_screens_preserves_stored_tool_choices() {
    let home = std::env::temp_dir().join(format!(
        "petri_s8_pty_tools_home_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".petridish")).expect("scratch home must be creatable");
    let prefs_path = home.join(".petridish").join("petri.toml");

    // A user who has already answered the picker for two actions.
    std::fs::write(
        &prefs_path,
        b"last_screen = \"dashboard\"\ncollapsed = [false, false, true, true]\n\n[tools]\nedit = \"code\"\ngitlog = \"serie\"\n",
    )
    .expect("seed prefs must be writable");

    let mut session = Session::spawn_with_home(&fixture_path("loaded.json"), 80, 40, &home);
    let screen = session.screen_retry(
        80,
        40,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
    );
    assert!(
        screen[0].contains("petri"),
        "petri must have started, got row 0: {:?}",
        screen[0]
    );

    // Tab is the cheapest key that triggers a prefs write.
    session.writer.write_all(b"\t").expect("write Tab must succeed");
    session.writer.flush().expect("flush must succeed");
    session.screen_retry(
        80,
        40,
        Duration::from_secs(5),
        Duration::from_millis(300),
        5,
    );

    session.writer.write_all(b"q").expect("write q must succeed");
    session.writer.flush().expect("flush must succeed");
    session.wait_with_timeout(Duration::from_secs(10));

    let written = std::fs::read_to_string(&prefs_path).expect("prefs file must still exist");
    assert!(
        written.contains("[tools]"),
        "the [tools] table was destroyed by a screen switch:\n{written}"
    );
    assert!(
        written.contains("edit = \"code\""),
        "the stored editor choice was lost:\n{written}"
    );
    assert!(
        written.contains("gitlog = \"serie\""),
        "the stored git-history choice was lost:\n{written}"
    );
    assert!(
        written.contains("last_screen = \"browser\""),
        "the Tab switch itself must still have been persisted:\n{written}"
    );
}
