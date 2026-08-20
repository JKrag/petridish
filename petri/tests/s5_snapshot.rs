//! S5 acceptance gate, layer 2 (petri/SPEC.md §3.1/§8, ADR-0003) — protected,
//! authored by the orchestrator, not the delegate. Same structural-not-exact
//! approach as S4's `s4_snapshot.rs` (see that file's module doc comment for
//! why) at the three geometries petri/SPEC.md §9 names for this slice:
//! 80×24, 200×50, 40×10.
//!
//! `browser::render`'s `todo!()` body means every test below FAILS (panics)
//! rather than errors — confirmed before delegating S5.

use petridish_core::schema::Radar;
use petri::browser::BrowserState;
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

fn rendered_lines(radar: &Radar, state: &BrowserState, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal must construct");
    terminal
        .draw(|frame| petri::browser::render(frame, radar, state))
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
#[ignore = "S5 gate: BrowserState/browser::render not implemented yet; run explicitly with --ignored"]
fn header_identifies_the_app_at_80x24() {
    let radar = load("normal.json");
    let state = BrowserState::new(&radar);
    let lines = rendered_lines(&radar, &state, 80, 24);
    assert!(lines[0].contains("petri"), "row 0 must contain \"petri\", got: {:?}", lines[0]);
}

#[test]
#[ignore = "S5 gate: BrowserState/browser::render not implemented yet; run explicitly with --ignored"]
fn section_labels_are_rendered_for_populated_buckets() {
    // petri/SPEC.md §3.1: "Grouped list, sections in the fixed order active,
    // in_flight, stale, cold, each with a header and a count. Section
    // labels: RUNNING / IN FLIGHT / STALE / COLD." This is the one assertion
    // in this file that S4's flat-list placeholder structurally cannot
    // satisfy (it has no concept of sections at all) — the rest of this
    // file's checks (header text, name reachability) happen to also hold for
    // a flat list, so this is what actually discriminates "S5 implemented"
    // from "still the S4 stub".
    let radar = load("normal.json"); // 5 active / 4 in_flight / 4 stale / 3 cold
    let state = BrowserState::new(&radar);
    let lines = rendered_lines(&radar, &state, 80, 40);
    let whole = lines.join("\n");
    for label in ["RUNNING", "IN FLIGHT", "STALE", "COLD"] {
        assert!(
            whole.contains(label),
            "normal.json populates every bucket, so section label {label:?} must be rendered somewhere, got:\n{whole}"
        );
    }
}

#[test]
#[ignore = "S5 gate: BrowserState/browser::render not implemented yet; run explicitly with --ignored"]
fn every_visible_project_name_reachable_at_200x50() {
    // 200x50 is roomy — normal.json's 15 projects must all fit and be visible.
    let radar = load("normal.json");
    let state = BrowserState::new(&radar);
    let lines = rendered_lines(&radar, &state, 200, 50);
    let whole = lines.join("\n");
    for &idx in &state.visible {
        let name = &radar.projects[idx].name;
        assert!(whole.contains(name.as_str()), "project {name:?} must be reachable at 200x50, got:\n{whole}");
    }
}

#[test]
#[ignore = "S5 gate: BrowserState/browser::render not implemented yet; run explicitly with --ignored"]
fn every_visible_project_name_reachable_at_80x24_minimal() {
    let radar = load("minimal.json");
    let state = BrowserState::new(&radar);
    let lines = rendered_lines(&radar, &state, 80, 24);
    let whole = lines.join("\n");
    for &idx in &state.visible {
        let name = &radar.projects[idx].name;
        assert!(whole.contains(name.as_str()), "project {name:?} must be reachable at 80x24, got:\n{whole}");
    }
}

#[test]
#[ignore = "S5 gate: BrowserState/browser::render not implemented yet; run explicitly with --ignored"]
fn detail_pane_absent_entirely_at_40x10() {
    // petri/SPEC.md §3.1: "If the window is too narrow to give it a usable
    // width, hide it entirely rather than squeezing." 40 columns is not
    // enough for a list column AND a usable detail pane side-by-side. We
    // can't directly assert "no detail pane widget was drawn" through a
    // buffer, so this asserts the *consequence*: the selected project's own
    // detail-only fields (github_url, session_id — never shown in the list
    // column, only in the detail pane per §3.1's field list) must NOT appear
    // anywhere on screen at this width, because there's nowhere to put them
    // without squeezing.
    let radar = load("normal.json");
    let state = BrowserState::new(&radar);
    let selected_idx = state.visible[state.selected.expect("normal.json is non-empty")];
    let selected = &radar.projects[selected_idx];

    let lines = rendered_lines(&radar, &state, 40, 10);
    let whole = lines.join("\n");

    if let Some(url) = &selected.git.github_url {
        assert!(
            !whole.contains(url.as_str()),
            "detail-only field (github_url) must not appear at 40x10 — the detail pane must be hidden, not squeezed, got:\n{whole}"
        );
    }
    if let Some(session_id) = &selected.agent.session_id {
        assert!(
            !whole.contains(session_id.as_str()),
            "detail-only field (session_id) must not appear at 40x10 — the detail pane must be hidden, not squeezed, got:\n{whole}"
        );
    }
}

#[test]
#[ignore = "S5 gate: BrowserState/browser::render not implemented yet; run explicitly with --ignored"]
fn does_not_panic_on_empty_visible_list() {
    let radar = Radar {
        schema_version: 1,
        updated_at: chrono::Utc::now(),
        scan_duration_ms: 0,
        projects: vec![],
        quota: None,
    };
    let state = BrowserState::new(&radar);
    // Renders a "nothing selected" state (petri/SPEC.md §3.1) — must not panic.
    let _ = rendered_lines(&radar, &state, 80, 24);
}

#[test]
#[ignore = "S5 gate: BrowserState/browser::render not implemented yet; run explicitly with --ignored"]
fn does_not_panic_at_tiny_geometry() {
    let radar = load("minimal.json");
    let state = BrowserState::new(&radar);
    let _ = rendered_lines(&radar, &state, 1, 1);
}
