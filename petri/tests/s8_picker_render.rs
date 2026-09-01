//! S8 gate: the picker popup actually draws (petri/IDEAS.md `MECH-1`).
//!
//! Structural buffer assertions via `TestBackend`, matching SPEC.md §8 layer 2
//! — content that must appear, not a pinned byte-for-byte buffer. Hermetic:
//! the candidates are constructed here rather than probed from the machine, so
//! this says the same thing on a laptop with four git TUIs and on a CI box
//! with none.

use petri::picker::PickerState;
use petri::tools::{Action, Candidate, ExecMode, Target};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use crossterm::event::KeyCode;

fn state() -> PickerState {
    let action = Action {
        id: "gitlog",
        key: 'g',
        label: "git history",
        target: Target::Path,
        candidates: vec![],
    };
    PickerState::new(
        &action,
        vec![
            Candidate::new("serie", &[], ExecMode::Terminal),
            Candidate::new("lazygit", &["-p", "{path}"], ExecMode::Terminal),
        ],
    )
}

fn draw(state: &PickerState, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
    terminal
        .draw(|frame| petri::picker::render(frame, state))
        .expect("draw must succeed");
    let buffer = terminal.backend().buffer().clone();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_popup_shows_the_action_title_and_every_option() {
    let screen = draw(&state(), 80, 24);
    assert!(screen.contains("git history"), "missing title:\n{screen}");
    assert!(screen.contains("serie"), "missing first candidate:\n{screen}");
    assert!(screen.contains("lazygit"), "missing second candidate:\n{screen}");
    assert!(screen.contains("Other"), "missing the escape hatch:\n{screen}");
}

#[test]
fn the_popup_tells_the_user_where_the_answer_is_stored() {
    // `ACT-8`'s footnote. Without it the only way to change a stored choice is
    // to already know the file exists, which is exactly the state the picker
    // was built to rescue the user from.
    let screen = draw(&state(), 80, 24);
    assert!(
        screen.contains("petri.toml"),
        "the popup must say where the choice is saved:\n{screen}"
    );
}

#[test]
fn the_popup_advertises_only_the_keys_bound_in_the_mode_it_is_in() {
    // SPEC.md §3.1's rule, applied to the popup: in list mode it offers
    // move/choose/cancel; once typing, "move" is a lie and must be gone.
    let list_mode = draw(&state(), 80, 24);
    assert!(list_mode.contains("j/k"), "list mode must offer movement:\n{list_mode}");

    let mut typing = state();
    for _ in 0..2 {
        typing.on_key(KeyCode::Down);
    }
    typing.on_key(KeyCode::Enter);
    let typed = draw(&typing, 80, 24);
    assert!(
        !typed.contains("j/k"),
        "movement must not be advertised while j and k are literal text:\n{typed}"
    );
    assert!(typed.contains("Esc back"), "expected the two-stage Esc hint:\n{typed}");
}

#[test]
fn the_typed_path_is_visible_as_it_is_entered() {
    let mut typing = state();
    for _ in 0..2 {
        typing.on_key(KeyCode::Down);
    }
    typing.on_key(KeyCode::Enter);
    for c in "jjui".chars() {
        typing.on_key(KeyCode::Char(c));
    }
    let screen = draw(&typing, 80, 24);
    assert!(screen.contains("jjui"), "typed text must be shown back:\n{screen}");
}

#[test]
fn the_popup_does_not_panic_on_a_tiny_terminal() {
    // Popups are the one widget that can be asked to draw larger than the
    // screen. A panic here would take the whole TUI down over a window resize.
    for (w, h) in [(20u16, 6u16), (10, 4), (80, 3)] {
        let _ = draw(&state(), w, h);
    }
}
