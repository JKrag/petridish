//! S8 acceptance gate, task 3 (petri/IDEAS.md `ACT-8`) — **protected,
//! authored by the orchestrator, not the delegate.**
//!
//! The complete specification of `picker::PickerState`. Pure state
//! transitions only: no terminal, no rendering, no `PATH`.
//!
//! Two rules here are load-bearing rather than cosmetic, and both have their
//! own tests below. Selection is **clamped, never wrapping**, matching the
//! Browser's own rule in petri/SPEC.md §3.1 — the two surfaces must not
//! disagree about what pressing `k` at the top does. And `j`/`k` are cursor
//! movement in list mode but **literal text** in custom-input mode: a picker
//! that ate the `j` out of a program name would be unusable for exactly the
//! user who needs the escape hatch.

use petri::picker::{Choice, Outcome, PickerState};
use petri::tools::{Action, Candidate, ExecMode, Target};

use crossterm::event::KeyCode;

fn action() -> Action {
    Action {
        id: "gitlog",
        key: 'g',
        label: "git history",
        target: Target::Path,
        candidates: vec![],
    }
}

fn candidates() -> Vec<Candidate> {
    vec![
        Candidate::new("serie", &[], ExecMode::Terminal),
        Candidate::new("lazygit", &["-p", "{path}"], ExecMode::Terminal),
        Candidate::new("git", &["log"], ExecMode::Terminal).as_fallback(),
    ]
}

fn open() -> PickerState {
    PickerState::new(&action(), candidates())
}

fn program_names(state: &PickerState) -> Vec<String> {
    state
        .options()
        .iter()
        .map(|o| match o {
            Choice::Candidate(c) => c.program.clone(),
            Choice::Other => "<other>".to_string(),
        })
        .collect()
}

// ------------------------------------------------------------ construction --

#[test]
fn options_are_the_installed_candidates_then_other() {
    let state = open();
    assert_eq!(
        program_names(&state),
        vec!["serie", "lazygit", "git", "<other>"],
        "registry order preserved, Other appended last"
    );
}

#[test]
fn the_cursor_starts_on_the_best_guess() {
    // The candidates arrive best-first, so starting at 0 is what makes the
    // popup a one-keystroke interaction (`ACT-8`: "order candidates
    // opinionatedly, so Enter does the sane thing").
    assert_eq!(open().selected(), 0);
}

#[test]
fn the_picker_carries_the_action_it_is_configuring() {
    // The id is the preferences key the answer gets stored under; the label
    // is the popup title. Getting the id wrong would silently store the
    // answer where nothing will ever read it.
    let state = open();
    assert_eq!(state.action_id, "gitlog");
    assert_eq!(state.title, "git history");
}

#[test]
fn a_machine_with_nothing_installed_still_offers_other() {
    // `Other` is never conditional. Without it, a user whose editor the
    // registry has never heard of would have no way in at all.
    let state = PickerState::new(&action(), vec![]);
    assert_eq!(program_names(&state), vec!["<other>"]);
    assert_eq!(state.selected(), 0);
}

#[test]
fn the_picker_starts_in_list_mode_not_typing_mode() {
    assert_eq!(open().custom_input(), None);
}

// -------------------------------------------------------------- movement ----

#[test]
fn down_and_j_both_move_the_cursor_down() {
    let mut state = open();
    assert_eq!(state.on_key(KeyCode::Down), Outcome::Pending);
    assert_eq!(state.selected(), 1);
    assert_eq!(state.on_key(KeyCode::Char('j')), Outcome::Pending);
    assert_eq!(state.selected(), 2);
}

#[test]
fn up_and_k_both_move_the_cursor_up() {
    let mut state = open();
    state.on_key(KeyCode::Down);
    state.on_key(KeyCode::Down);
    assert_eq!(state.on_key(KeyCode::Up), Outcome::Pending);
    assert_eq!(state.selected(), 1);
    assert_eq!(state.on_key(KeyCode::Char('k')), Outcome::Pending);
    assert_eq!(state.selected(), 0);
}

#[test]
fn selection_clamps_at_the_top_and_does_not_wrap() {
    // SPEC.md §3.1's rule for the Browser, applied here so the two surfaces
    // agree. Wrapping would teleport the cursor from the first real tool to
    // "Other", which is the single most destructive row to land on by
    // accident.
    let mut state = open();
    for _ in 0..5 {
        state.on_key(KeyCode::Up);
    }
    assert_eq!(state.selected(), 0);
}

#[test]
fn selection_clamps_at_the_bottom_and_does_not_wrap() {
    let mut state = open();
    for _ in 0..20 {
        state.on_key(KeyCode::Down);
    }
    assert_eq!(
        state.selected(),
        3,
        "four options means the last index is 3, and it must stay there"
    );
}

// ---------------------------------------------------------------- choosing --

#[test]
fn enter_on_a_candidate_chooses_its_program_name() {
    let mut state = open();
    state.on_key(KeyCode::Down);
    assert_eq!(
        state.on_key(KeyCode::Enter),
        Outcome::Chosen("lazygit".to_string()),
        "the answer is the bare program name — what gets stored in [tools]"
    );
}

#[test]
fn enter_on_the_first_row_chooses_without_any_movement() {
    let mut state = open();
    assert_eq!(state.on_key(KeyCode::Enter), Outcome::Chosen("serie".to_string()));
}

#[test]
fn a_fallback_is_choosable_like_any_other_row() {
    // A fallback never *triggers* the picker, but it is a legitimate answer
    // once the picker is open: someone with lazygit installed may still
    // prefer plain `git log`.
    let mut state = open();
    state.on_key(KeyCode::Down);
    state.on_key(KeyCode::Down);
    assert_eq!(state.on_key(KeyCode::Enter), Outcome::Chosen("git".to_string()));
}

#[test]
fn esc_in_list_mode_cancels() {
    let mut state = open();
    assert_eq!(state.on_key(KeyCode::Esc), Outcome::Cancelled);
}

// ------------------------------------------------------------ custom input --

#[test]
fn enter_on_other_opens_an_empty_text_input_rather_than_choosing() {
    let mut state = open();
    for _ in 0..3 {
        state.on_key(KeyCode::Down);
    }
    assert_eq!(
        state.on_key(KeyCode::Enter),
        Outcome::Pending,
        "Other is not itself an answer — it opens the field where the answer is typed"
    );
    assert_eq!(state.custom_input(), Some(""));
}

fn open_in_custom_mode() -> PickerState {
    let mut state = open();
    for _ in 0..3 {
        state.on_key(KeyCode::Down);
    }
    state.on_key(KeyCode::Enter);
    state
}

#[test]
fn typed_characters_accumulate() {
    let mut state = open_in_custom_mode();
    for c in "tig".chars() {
        assert_eq!(state.on_key(KeyCode::Char(c)), Outcome::Pending);
    }
    assert_eq!(state.custom_input(), Some("tig"));
}

#[test]
fn j_and_k_are_text_in_custom_mode_not_movement() {
    // THE rule this mode flag exists for. `k` and `j` appear in real program
    // names (`kak`, `jj`, `/usr/local/bin/jjui`); eating them as cursor keys
    // would make the escape hatch unusable for the very users who need it.
    let mut state = open_in_custom_mode();
    let before = state.selected();
    for c in "kak".chars() {
        state.on_key(KeyCode::Char(c));
    }
    assert_eq!(state.custom_input(), Some("kak"));
    assert_eq!(
        state.selected(),
        before,
        "typing must not move the list cursor underneath the input"
    );
}

#[test]
fn arrow_keys_do_not_move_the_cursor_in_custom_mode_either() {
    // Distinct from the j/k case: those are ambiguous characters, these are
    // unambiguous cursor keys that still must not act on a list the user is
    // no longer looking at.
    let mut state = open_in_custom_mode();
    let before = state.selected();
    state.on_key(KeyCode::Up);
    state.on_key(KeyCode::Down);
    assert_eq!(state.selected(), before);
    assert_eq!(state.custom_input(), Some(""), "cursor keys are not text either");
}

#[test]
fn backspace_deletes_the_last_character() {
    let mut state = open_in_custom_mode();
    for c in "tig".chars() {
        state.on_key(KeyCode::Char(c));
    }
    assert_eq!(state.on_key(KeyCode::Backspace), Outcome::Pending);
    assert_eq!(state.custom_input(), Some("ti"));
}

#[test]
fn backspace_on_an_empty_input_is_harmless() {
    let mut state = open_in_custom_mode();
    assert_eq!(state.on_key(KeyCode::Backspace), Outcome::Pending);
    assert_eq!(state.custom_input(), Some(""), "must not underflow or exit the mode");
}

#[test]
fn enter_accepts_the_typed_program() {
    let mut state = open_in_custom_mode();
    for c in "jjui".chars() {
        state.on_key(KeyCode::Char(c));
    }
    assert_eq!(state.on_key(KeyCode::Enter), Outcome::Chosen("jjui".to_string()));
}

#[test]
fn enter_on_an_empty_input_does_not_accept_an_empty_program_name() {
    // Storing "" would be worse than storing nothing: `resolve` would treat
    // it as a configured answer, fail `installed("")`, and quietly fall
    // through forever while the user believed they had answered.
    let mut state = open_in_custom_mode();
    assert_eq!(state.on_key(KeyCode::Enter), Outcome::Pending);
    assert_eq!(state.custom_input(), Some(""), "still typing, not cancelled");
}

#[test]
fn a_whitespace_only_input_is_not_a_program_name_either() {
    let mut state = open_in_custom_mode();
    state.on_key(KeyCode::Char(' '));
    state.on_key(KeyCode::Char(' '));
    assert_eq!(state.on_key(KeyCode::Enter), Outcome::Pending);
}

#[test]
fn the_accepted_program_name_is_trimmed() {
    let mut state = open_in_custom_mode();
    for c in "  tig ".chars() {
        state.on_key(KeyCode::Char(c));
    }
    assert_eq!(state.on_key(KeyCode::Enter), Outcome::Chosen("tig".to_string()));
}

#[test]
fn esc_in_custom_mode_returns_to_the_list_rather_than_cancelling() {
    // Two-stage escape. A single Esc backing all the way out would punish a
    // mis-hit on Other by throwing away the whole interaction; here it just
    // undoes the mis-hit, and a second Esc cancels for real.
    let mut state = open_in_custom_mode();
    for c in "tig".chars() {
        state.on_key(KeyCode::Char(c));
    }
    assert_eq!(state.on_key(KeyCode::Esc), Outcome::Pending);
    assert_eq!(state.custom_input(), None, "back in list mode");
    assert_eq!(state.on_key(KeyCode::Esc), Outcome::Cancelled, "the second Esc cancels");
}

#[test]
fn leaving_custom_mode_discards_what_was_typed() {
    let mut state = open_in_custom_mode();
    for c in "tig".chars() {
        state.on_key(KeyCode::Char(c));
    }
    state.on_key(KeyCode::Esc);
    state.on_key(KeyCode::Enter);
    assert_eq!(
        state.custom_input(),
        Some(""),
        "re-entering must start empty, not resume an abandoned draft"
    );
}

#[test]
fn the_list_cursor_survives_a_trip_through_custom_mode() {
    let mut state = open_in_custom_mode();
    state.on_key(KeyCode::Esc);
    assert_eq!(state.selected(), 3, "still on Other, where the user left it");
}

// ------------------------------------------------------------ inert keys ----

#[test]
fn unbound_keys_in_list_mode_do_nothing() {
    // The footer advertises only bound keys (SPEC.md §3.1); anything else
    // must be inert rather than surprising. In particular a stray character
    // must not start editing a program name.
    let mut state = open();
    assert_eq!(state.on_key(KeyCode::Char('z')), Outcome::Pending);
    assert_eq!(state.selected(), 0);
    assert_eq!(state.custom_input(), None);
    assert_eq!(state.on_key(KeyCode::Tab), Outcome::Pending);
    assert_eq!(state.custom_input(), None);
}
