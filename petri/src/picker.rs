//! The first-run tool picker's state machine (`petri/IDEAS.md` `ACT-8`).
//!
//! When an action has several plausible programs on this machine — four git
//! TUIs installed, or `code` and `nvim` both on `PATH` — `petri` neither
//! guesses nor makes the user find the config file first. It opens a popup
//! (`MECH-1`), asks once, stores the answer in the preferences file's
//! `[tools]` table, and never asks again.
//!
//! This module is the pure half: which options exist, where the cursor is,
//! what a keystroke does, and what the answer finally is. Rendering the popup
//! is `dashboard.rs`/`browser.rs`'s business, and persisting the answer is
//! `prefs.rs`'s. Nothing here reads `PATH`, the environment, or the
//! preferences file — the installed candidates arrive as a parameter, from
//! [`crate::tools::Resolution::Ambiguous`].
//!
//! The picker is only *opened* on a genuinely ambiguous resolution, never on a
//! sole candidate. That decision belongs to `tools::resolve`, not here — see
//! `Candidate::fallback` for why it matters that a last-resort entry can't
//! trigger this popup.

use crate::tools::{Action, Candidate};
use crossterm::event::KeyCode;

/// One row in the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// A program the registry knows about and that is installed here.
    Candidate(Candidate),
    /// `Other — specify path…`. Always present, always last: it is the escape
    /// hatch for a machine whose preferred tool the registry has never heard
    /// of, and the reason a `nano` user is never stuck with a bad guess.
    Other,
}

/// What a keystroke did to the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The picker is still open. Keep rendering it.
    Pending,
    /// The user picked this program name. The caller stores it under the
    /// action's id in the preferences file and then launches it.
    Chosen(String),
    /// The user backed out. Nothing is stored and nothing is launched — and
    /// crucially the picker must open again next time, rather than recording
    /// "no tool" as if it were an answer.
    Cancelled,
}

/// The picker's whole state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerState {
    /// The action being configured — the key this answer is stored under.
    pub action_id: &'static str,
    /// The action's human label, for the popup title.
    pub title: &'static str,
    options: Vec<Choice>,
    selected: usize,
    /// `Some` while the user is typing a custom program name, `None` while
    /// they are moving through the list. This is the mode flag, and it is
    /// load-bearing: `j`/`k` are cursor movement in list mode and literal
    /// text in custom mode, and confusing the two is the same class of bug as
    /// letting an action key fire while the Browser's `/` filter has focus.
    custom: Option<String>,
}

impl PickerState {
    /// Open the picker for `action`, offering `installed` (which comes
    /// straight from [`crate::tools::Resolution::Ambiguous`], already filtered
    /// to what is present on this machine and already in registry order —
    /// best guess first, so the cursor starts on it and `Enter` is the whole
    /// interaction).
    pub fn new(action: &Action, installed: Vec<Candidate>) -> Self {
        // One row per installed candidate, in registry order, then the
        // always-present `Other` escape hatch last.
        let options: Vec<Choice> = installed
            .into_iter()
            .map(Choice::Candidate)
            .chain(std::iter::once(Choice::Other))
            .collect();
        Self {
            action_id: action.id,
            title: action.label,
            options,
            selected: 0,
            custom: None,
        }
    }

    /// Every row, in display order, `Other` last.
    pub fn options(&self) -> &[Choice] {
        &self.options
    }

    /// Index of the highlighted row.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The custom program name typed so far, or `None` when the user is
    /// moving through the list rather than typing.
    pub fn custom_input(&self) -> Option<&str> {
        self.custom.as_deref()
    }

    /// Feed one keystroke in.
    pub fn on_key(&mut self, key: KeyCode) -> Outcome {
        // The mode flag decides everything: `j`/`k` and the arrow keys are
        // cursor movement while the user browses the list, but literal text
        // (or nothing at all) while they are typing a custom program name.
        // Confusing the two is the bug this flag exists to prevent, so the mode
        // is checked inside every key that could act in either mode.
        match key {
            // --- Cursor movement: list mode only. ---
            KeyCode::Up => {
                if self.custom.is_none() {
                    // Clamp at the top — never wrap onto `Other`.
                    self.selected = self.selected.saturating_sub(1);
                }
                Outcome::Pending
            }
            KeyCode::Down => {
                if self.custom.is_none() {
                    // Clamp at the last row — never wrap back to the top.
                    self.selected = self.selected.saturating_add(1).min(self.options.len() - 1);
                }
                Outcome::Pending
            }
            // --- Choosing: the meaning of Enter depends on the mode. ---
            KeyCode::Enter => {
                if self.custom.is_some() {
                    // Custom mode: accept the trimmed name, or stay put when it
                    // is empty (an empty program name is never an answer).
                    let trimmed = self.custom.as_deref().unwrap_or("").trim();
                    if trimmed.is_empty() {
                        Outcome::Pending
                    } else {
                        Outcome::Chosen(trimmed.to_string())
                    }
                } else {
                    // List mode: pick the highlighted row, or open the text
                    // field when the highlight is `Other`.
                    match &self.options[self.selected] {
                        Choice::Candidate(c) => Outcome::Chosen(c.program.clone()),
                        Choice::Other => {
                            self.custom = Some(String::new());
                            Outcome::Pending
                        }
                    }
                }
            }
            // --- Editing the program name: custom mode. ---
            KeyCode::Backspace => {
                // Drop the last character, but never underflow or exit the mode
                // on an already-empty input.
                if let Some(text) = self.custom.as_mut() {
                    if !text.is_empty() {
                        text.pop();
                    }
                }
                Outcome::Pending
            }
            // --- A literal character: text in custom mode, movement in list mode. ---
            KeyCode::Char(c) => {
                if let Some(text) = self.custom.as_mut() {
                    // Custom mode: every character is literal text, including the
                    // `k`/`j` cursor keys — real program names contain them.
                    text.push(c);
                } else if c == 'k' {
                    // List mode: `k` moves up (clamped, never wrapping).
                    self.selected = self.selected.saturating_sub(1);
                } else if c == 'j' {
                    // List mode: `j` moves down (clamped, never wrapping).
                    self.selected = self.selected.saturating_add(1).min(self.options.len() - 1);
                }
                // Any other character in list mode is inert.
                Outcome::Pending
            }
            // --- Escaping: the meaning depends on the mode too. ---
            KeyCode::Esc => {
                if self.custom.is_none() {
                    // Already in list mode: back all the way out.
                    Outcome::Cancelled
                } else {
                    // Just entered the field: undo that instead of cancelling.
                    // Whatever was typed is discarded, so re-entering starts empty.
                    self.custom = None;
                    Outcome::Pending
                }
            }
            // Any other key (including stray `z`/`Tab`) is inert.
            _ => Outcome::Pending,
        }
    }
}
