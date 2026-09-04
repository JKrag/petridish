//! S8 acceptance gate, task 1 (petri/IDEAS.md `ACT-8`, petri/SPEC.md §6) —
//! **protected, authored by the orchestrator, not the delegate.**
//!
//! `ACT-8` (the first-run tool picker) is only worth building if the answer
//! survives a restart, so the picker's storage is specified and gated before
//! the picker itself exists. This is the `[tools]` table of the preferences
//! file: a map from an action id (`"gitlog"`, `"edit"`, `"browse"` — the
//! `Action::id` values `tools.rs` will define in task 2) to the program the
//! user chose for it.
//!
//! Deliberately decoupled from `tools.rs`: the value is a plain `String`
//! program name, not a `Candidate` or any other type from the registry. The
//! preferences file must stay parseable by a `petri` whose registry has since
//! gained, lost or renamed candidates — so it stores the user's answer, never
//! a snapshot of the menu they picked it from.
//!
//! §6's existing contract is unchanged and re-gated here: missing file ->
//! defaults, corrupt file -> defaults plus a warning, never a crash. See
//! `s7_prefs.rs` for the pre-`[tools]` half of that contract; nothing there
//! may regress.

use petri::prefs::{self, LastScreen, Prefs};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn scratch_path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "petri_s8_prefs_tools_{name}_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir must be creatable");
    dir.join("petri.toml")
}

fn choices(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

#[test]
fn default_prefs_have_no_tool_choices() {
    // First run must not pre-answer the picker on the user's behalf. An empty
    // map is what makes `ACT-8` fire at all; a populated default would mean
    // the popup never opens and the user never gets a say.
    assert!(
        Prefs::default().tools.is_empty(),
        "a fresh Prefs must carry zero tool choices, got {:?}",
        Prefs::default().tools
    );
}

#[test]
fn prefs_file_written_before_tools_existed_still_parses() {
    // Schema drift, forward direction: a petri.toml written by a build that
    // predates `[tools]` has no such table. It must parse cleanly with an
    // empty map — not fail, and not discard the fields it *does* carry.
    let path = scratch_path("prefs_file_written_before_tools_existed_still_parses");
    std::fs::write(
        &path,
        b"last_screen = \"browser\"\ncollapsed = [true, false, true, false]\n",
    )
    .expect("write legacy prefs must succeed");

    let prefs = prefs::load(&path);
    assert!(
        prefs.tools.is_empty(),
        "absent [tools] must mean no choices"
    );
    assert_eq!(
        prefs.last_screen,
        LastScreen::Browser,
        "the pre-existing fields must survive the new one being added"
    );
    assert_eq!(prefs.collapsed, [true, false, true, false]);
}

#[test]
fn roundtrip_preserves_tool_choices() {
    let path = scratch_path("roundtrip_preserves_tool_choices");
    let written = Prefs {
        last_screen: LastScreen::Browser,
        tools: choices(&[("gitlog", "serie"), ("edit", "code")]),
        ..Prefs::default()
    };

    prefs::save(&path, &written).expect("save must succeed");
    let read_back = prefs::load(&path);

    assert_eq!(
        read_back, written,
        "prefs must survive a save/load round trip"
    );
    assert_eq!(
        read_back.tools.get("gitlog").map(String::as_str),
        Some("serie")
    );
    assert_eq!(
        read_back.tools.get("edit").map(String::as_str),
        Some("code")
    );
}

#[test]
fn tool_choices_serialize_as_a_real_toml_table() {
    // Asserted on the file text, not just on a round trip: `[tools]` must be a
    // table a human can hand-edit, because the picker's own footnote points the
    // user at this file (`ACT-8`). The on-disk key name is therefore part of the
    // contract, not an implementation detail — a round-trip test would happily
    // stay green while writing the table under some other name. Mutation-probed:
    // renaming the serialized field turns this test (and two others) red.
    //
    // A note on what this test does NOT gate, since an earlier draft claimed it
    // did: field declaration order is not a correctness hazard here. toml 0.5-era
    // serializers errored with `ValueAfterTable` when a map preceded a scalar;
    // the toml 0.8 backend this crate pins does not, confirmed by direct
    // experiment. `tools` is still declared last in `Prefs`, but for readability
    // only.
    let path = scratch_path("tool_choices_serialize_as_a_real_toml_table");
    let written = Prefs {
        tools: choices(&[("gitlog", "lazygit")]),
        ..Prefs::default()
    };
    prefs::save(&path, &written).expect("save must succeed — a ValueAfterTable error here means the map field is not declared last in Prefs");

    let text = std::fs::read_to_string(&path).expect("written prefs must be readable");
    assert!(
        text.contains("[tools]"),
        "expected a [tools] table header in:\n{text}"
    );
    assert!(
        text.contains("gitlog = \"lazygit\""),
        "expected the choice as a plain key = string in:\n{text}"
    );
}

#[test]
fn unknown_action_ids_round_trip_untouched() {
    // Backward direction of schema drift: a prefs file naming an action this
    // build has never heard of must not be dropped on the next save. Dropping
    // it would silently discard the user's answer when they downgrade, then
    // re-ask them for it on the next upgrade.
    let path = scratch_path("unknown_action_ids_round_trip_untouched");
    std::fs::write(
        &path,
        b"[tools]\nsomething_from_the_future = \"quux\"\ngitlog = \"tig\"\n",
    )
    .expect("write must succeed");

    let loaded = prefs::load(&path);
    assert_eq!(
        loaded
            .tools
            .get("something_from_the_future")
            .map(String::as_str),
        Some("quux"),
        "an unrecognised action id must be preserved verbatim"
    );

    let path2 = scratch_path("unknown_action_ids_round_trip_untouched_2");
    prefs::save(&path2, &loaded).expect("save must succeed");
    assert_eq!(prefs::load(&path2).tools, loaded.tools);
}

#[test]
fn malformed_tools_table_falls_back_to_defaults_not_a_panic() {
    // §6's standing contract, re-gated against the new field: a `[tools]`
    // value of the wrong type is a parse error for the whole document, and a
    // parse error means defaults plus a warning — never a crash, never a
    // refusal to start.
    let path = scratch_path("malformed_tools_table_falls_back_to_defaults_not_a_panic");
    std::fs::write(
        &path,
        b"last_screen = \"browser\"\ntools = \"not a table\"\n",
    )
    .expect("write must succeed");

    let prefs = prefs::load(&path);
    assert_eq!(
        prefs,
        Prefs::default(),
        "a type-mismatched tools value must degrade to full defaults"
    );
}
