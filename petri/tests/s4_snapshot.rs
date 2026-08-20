//! S4 acceptance gate, layer 2 (petri/SPEC.md §8/§9, ADR-0003) — protected,
//! authored by the orchestrator, not the delegate.
//!
//! Deliberately asserts STRUCTURE, not an exact `assert_buffer_lines` golden.
//! ADR-0003 describes layer-2 goldens as "self-referential" — committed expected
//! buffers for *this* implementation — which means the exact buffer can only be
//! captured honestly AFTER a real renderer exists and has been eyeballed once.
//! Pre-authoring an exact-buffer assertion here would mean writing the renderer's
//! layout in prose inside this test, which is both brittle against ratatui's own
//! layout decisions and not this test's job. So this gate checks the two things
//! that must be true regardless of exact layout — a header identifying the app,
//! and every project's name reachable somewhere on screen — and leaves the
//! pixel-exact golden to be captured as a separate follow-up test once S4 lands
//! and a human has looked at one real frame (ADR-0003 layer 4).
//!
//! Was `#[ignore]`d while `petri::app::render` was a `todo!()` stub (every test
//! below failed on that basis, confirmed before delegating S4); stripped now
//! that S4 landed and all five pass.

use petridish_core::schema::Radar;
use ratatui::{Terminal, backend::TestBackend};
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn load(name: &str) -> Radar {
    let text = std::fs::read_to_string(fixture_path(name))
        .unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("fixture {name} failed to deserialize into Radar: {e}"))
}

fn rendered_lines(radar: &Radar, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal must construct");
    terminal
        .draw(|frame| petri::app::render(frame, radar))
        .expect("draw must not error");
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect::<String>()
        })
        .collect()
}

#[test]
fn header_row_identifies_the_app() {
    let radar = load("minimal.json");
    let lines = rendered_lines(&radar, 80, 24);
    assert!(
        lines[0].contains("petri"),
        "row 0 must identify the app (contain \"petri\"), got: {:?}",
        lines[0]
    );
}

#[test]
fn every_project_name_is_reachable_on_screen_minimal() {
    let radar = load("minimal.json");
    let lines = rendered_lines(&radar, 80, 24);
    let whole_screen = lines.join("\n");
    for project in &radar.projects {
        assert!(
            whole_screen.contains(&project.name),
            "project name {:?} must appear somewhere on screen, got:\n{whole_screen}",
            project.name
        );
    }
}

#[test]
fn every_project_name_is_reachable_on_screen_normal_at_tall_geometry() {
    // normal.json has 15 projects (petri/SPEC.md §8) — give it enough rows that
    // "doesn't fit" isn't a legitimate excuse for a missing name.
    let radar = load("normal.json");
    let lines = rendered_lines(&radar, 80, 60);
    let whole_screen = lines.join("\n");
    for project in &radar.projects {
        assert!(
            whole_screen.contains(&project.name),
            "project name {:?} must appear somewhere on screen at 80x60, got:\n{whole_screen}",
            project.name
        );
    }
}

#[test]
fn empty_project_list_does_not_panic() {
    let radar = Radar {
        schema_version: 1,
        updated_at: chrono::Utc::now(),
        scan_duration_ms: 0,
        projects: vec![],
        quota: None,
    };
    // Must not panic — an empty *parsed* file is a valid state, distinct from a
    // missing state file (which petri::run rejects before this is ever called).
    let _ = rendered_lines(&radar, 80, 24);
}

#[test]
fn tiny_geometry_does_not_panic() {
    let radar = load("minimal.json");
    // Smallest TestBackend allows (0x0 is only reachable via a real forked pty,
    // covered instead by petri/tests/s4_pty.rs's resize test).
    let _ = rendered_lines(&radar, 1, 1);
}
