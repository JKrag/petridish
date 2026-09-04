//! Structural edits to `~/.claude/settings.json` (ARCHITECTURE.md §8.3 D4).
//!
//! This file is **not ours**. It is shared with every other Claude Code hook
//! consumer on the machine (pixtuoid, statusbar, notchbar, whatever the user
//! added last week), and it has no schema we own. So every operation here is
//! *structural*: we add list elements carrying `HOOK_MARKER`, we remove list
//! elements carrying `HOOK_MARKER`, and we pass literally everything else
//! through untouched. Nothing is normalised, reshaped, or validated.
//!
//! Key order is preserved rather than sorted. `serde_json`'s `preserve_order`
//! feature is enabled workspace-wide (see the root `Cargo.toml` for why it is
//! declared there); without it a `Map` is a `BTreeMap` and the first install
//! would silently alphabetise every key in a stranger's settings file. The
//! `add`-then-`remove` round-trip test below is what actually holds that
//! property down.

use petridish_core::schema::{HOOK_EVENTS, HOOK_MARKER};
use serde_json::{Map, Value};

/// Does `marker` appear anywhere in this subtree, at any depth, inside any
/// string? Ported from `installer.py::_contains_marker`.
fn contains_marker(value: &Value, marker: &str) -> bool {
    match value {
        Value::String(s) => s.contains(marker),
        Value::Array(items) => items.iter().any(|v| contains_marker(v, marker)),
        Value::Object(map) => map.values().any(|v| contains_marker(v, marker)),
        _ => false,
    }
}

/// Whole-file question: is any of our wiring present at all? This is what
/// `uninstall` and `doctor` ask.
pub fn has_marker(settings: &Value, marker: &str) -> bool {
    contains_marker(settings, marker)
}

/// Per-event question: is one of *our* hook groups registered under `event`?
pub fn event_has_marker(settings: &Value, event: &str, marker: &str) -> bool {
    settings
        .get("hooks")
        .and_then(|h| h.get(event))
        .map(|groups| contains_marker(groups, marker))
        .unwrap_or(false)
}

/// The `command` string we write. The marker is a trailing comment on the
/// command line, which is how the entry stays identifiable as ours.
pub fn hook_command(hook_abspath: &str, marker: &str) -> String {
    format!("{hook_abspath} {marker}")
}

/// Add our hook group under every event in `HOOK_EVENTS` that lacks one.
///
/// Returns `None` when there was nothing to do, so the caller can say "left
/// untouched" rather than rewriting an unchanged file. That mirrors
/// `installer.py`'s `updated is settings` identity check.
///
/// **Idempotent per event, not per file**, and that distinction is the whole
/// point. The original check short-circuited on the marker appearing anywhere,
/// so a user who installed before `Notification`/`PermissionRequest` were added
/// to `HOOK_EVENTS` would re-run `install`, be told "already installed", and
/// silently never receive the two new events — leaving the "waiting on you"
/// feature live in the code and dead on the only machine that had ever
/// installed it. Checking per event is what makes growing `HOOK_EVENTS` an
/// upgrade rather than a fresh-install-only feature.
pub fn add_hook_entries(settings: &Value, hook_abspath: &str, marker: &str) -> Option<Value> {
    let missing: Vec<&str> = HOOK_EVENTS
        .iter()
        .copied()
        .filter(|e| !event_has_marker(settings, e, marker))
        .collect();
    if missing.is_empty() {
        return None;
    }

    let mut updated = settings.as_object().cloned().unwrap_or_default();
    let mut hooks = updated
        .get("hooks")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let command = hook_command(hook_abspath, marker);
    for event in missing {
        let mut group = Map::new();
        let mut entry = Map::new();
        entry.insert("type".into(), Value::String("command".into()));
        entry.insert("command".into(), Value::String(command.clone()));
        group.insert("hooks".into(), Value::Array(vec![Value::Object(entry)]));

        let mut existing = hooks
            .get(event)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        existing.push(Value::Object(group));
        hooks.insert(event.to_string(), Value::Array(existing));
    }

    updated.insert("hooks".into(), Value::Object(hooks));
    Some(Value::Object(updated))
}

/// Is `item` a *dict* whose subtree carries the marker? Only dict elements of a
/// list are ever dropped — exactly the shape `add_hook_entries` adds.
fn is_marked_group(item: &Value, marker: &str) -> bool {
    item.is_object() && contains_marker(item, marker)
}

/// Recursively drop marked list elements, preserving everything else.
fn drop_marked(value: &Value, marker: &str) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .filter(|item| !is_marked_group(item, marker))
                .map(|item| drop_marked(item, marker))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), drop_marked(v, marker)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Remove every hook group we added, and nothing else.
///
/// Sibling entries from other consumers, and every key we did not write, pass
/// through unchanged — uninstall must never restore from the backup for exactly
/// this reason (D4): the backup predates any edits those consumers made since.
///
/// An event key we were the *sole* consumer of is dropped along with its last
/// group, so uninstall leaves no `"PermissionRequest": []` residue. The
/// exception matters: a key that was **already** empty before the drop is kept,
/// because that is a deliberate choice by the user and not something we
/// emptied.
pub fn remove_marker_entries(settings: &Value, marker: &str) -> Value {
    let cleaned = drop_marked(settings, marker);

    let (Some(before), Some(after)) = (
        settings.get("hooks").and_then(Value::as_object),
        cleaned.get("hooks").and_then(Value::as_object),
    ) else {
        return cleaned;
    };

    let pruned: Map<String, Value> = after
        .iter()
        .filter(|(event, groups)| {
            let is_empty_now = groups.as_array().map(|a| a.is_empty()).unwrap_or(false);
            let was_empty_before = before
                .get(*event)
                .and_then(Value::as_array)
                .map(|a| a.is_empty())
                .unwrap_or(false);
            !is_empty_now || was_empty_before
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let mut out = cleaned.as_object().cloned().unwrap_or_default();
    out.insert("hooks".into(), Value::Object(pruned));
    Value::Object(out)
}

/// `json.dumps(data, indent=2) + "\n"`.
///
/// One divergence from the Python, deliberate and recorded: `json.dumps`
/// defaults to `ensure_ascii=True` and escaped non-ASCII as `\uXXXX`, whereas
/// serde_json emits raw UTF-8. Both are valid JSON and parse identically; the
/// UTF-8 form is simply more readable in a file a human may open.
pub fn serialize_settings(value: &Value) -> String {
    format!(
        "{}\n",
        serde_json::to_string_pretty(value).unwrap_or_default()
    )
}

pub fn default_marker() -> &'static str {
    HOOK_MARKER
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HOOK: &str = "/Users/x/.cargo/bin/swab-hook";

    /// Shaped after a real `settings.json`: several unrelated consumers, a
    /// non-hook top-level key, and events we also register on.
    fn other_consumers() -> Value {
        json!({
            "model": "opus",
            "hooks": {
                "PreToolUse": [
                    {"hooks": [{"type": "command", "command": "/usr/local/bin/rtk-hook"}]},
                    {"matcher": "*", "hooks": [{"type": "command", "command": "/opt/notchbar/hook"}]}
                ],
                "Stop": [
                    {"hooks": [{"type": "command", "command": "/opt/pixtuoid/stop"}]}
                ]
            },
            "statusLine": {"type": "command", "command": "/opt/statusbar/line"}
        })
    }

    fn marker() -> &'static str {
        HOOK_MARKER
    }

    #[test]
    fn add_registers_every_event_without_disturbing_other_consumers() {
        let before = other_consumers();
        let after = add_hook_entries(&before, HOOK, marker()).expect("must change");

        for event in HOOK_EVENTS {
            assert!(
                event_has_marker(&after, event, marker()),
                "{event} must carry our group"
            );
        }

        // Every pre-existing group survives, in place.
        let hooks = after.get("hooks").unwrap();
        let pre = hooks.get("PreToolUse").unwrap().as_array().unwrap();
        assert_eq!(pre.len(), 3, "two existing + one ours");
        assert_eq!(pre[0], before["hooks"]["PreToolUse"][0]);
        assert_eq!(pre[1], before["hooks"]["PreToolUse"][1]);
        // Unrelated top-level keys are untouched.
        assert_eq!(after["model"], before["model"]);
        assert_eq!(after["statusLine"], before["statusLine"]);
    }

    #[test]
    fn add_is_a_no_op_once_every_event_is_registered() {
        let once = add_hook_entries(&other_consumers(), HOOK, marker()).unwrap();
        assert!(
            add_hook_entries(&once, HOOK, marker()).is_none(),
            "a second install must report nothing to do"
        );
    }

    /// The upgrade path, and the reason `add_hook_entries` checks per event.
    ///
    /// This is the exact state of every machine installed before MECH-5 added
    /// `Notification` and `PermissionRequest` to `HOOK_EVENTS`: our marker is
    /// present, but only under the original two events. A whole-file marker
    /// check reports "already installed" here and strands the two new events
    /// forever.
    #[test]
    fn add_upgrades_a_pre_mech5_install_without_duplicating_the_old_entries() {
        let mut pre_mech5 = other_consumers();
        let cmd = hook_command(HOOK, marker());
        for event in ["PreToolUse", "Stop"] {
            let groups = pre_mech5["hooks"][event].as_array_mut().unwrap();
            groups.push(json!({"hooks": [{"type": "command", "command": cmd}]}));
        }

        let after = add_hook_entries(&pre_mech5, HOOK, marker())
            .expect("the two MECH-5 events are missing, so this must change");

        // The new pair arrives...
        for event in ["Notification", "PermissionRequest"] {
            assert!(event_has_marker(&after, event, marker()), "{event} missing");
        }
        // ...and the already-registered pair is not duplicated.
        for event in ["PreToolUse", "Stop"] {
            let ours = after["hooks"][event]
                .as_array()
                .unwrap()
                .iter()
                .filter(|g| contains_marker(g, marker()))
                .count();
            assert_eq!(
                ours, 1,
                "{event} must still carry exactly one of our groups"
            );
        }
    }

    #[test]
    fn remove_drops_only_our_groups() {
        let before = other_consumers();
        let installed = add_hook_entries(&before, HOOK, marker()).unwrap();
        let cleaned = remove_marker_entries(&installed, marker());

        assert!(!has_marker(&cleaned, marker()));
        assert_eq!(
            cleaned["hooks"]["PreToolUse"], before["hooks"]["PreToolUse"],
            "other consumers' PreToolUse groups must be byte-identical"
        );
        assert_eq!(cleaned["hooks"]["Stop"], before["hooks"]["Stop"]);
        assert_eq!(cleaned["statusLine"], before["statusLine"]);
    }

    /// The property that proves nothing is being reshaped or reordered behind
    /// the user's back — and the canary for `preserve_order` being disabled.
    #[test]
    fn add_then_remove_is_byte_identical() {
        let before = other_consumers();
        let installed = add_hook_entries(&before, HOOK, marker()).unwrap();
        let cleaned = remove_marker_entries(&installed, marker());
        assert_eq!(
            serialize_settings(&cleaned),
            serialize_settings(&before),
            "install followed by uninstall must leave the file exactly as found"
        );
    }

    #[test]
    fn key_order_of_unrelated_keys_survives_a_round_trip() {
        // Deliberately not alphabetical: `zeta` before `alpha` proves we are
        // preserving insertion order rather than sorting.
        let before: Value =
            serde_json::from_str(r#"{"zeta": 1, "alpha": 2, "model": "opus"}"#).unwrap();
        let installed = add_hook_entries(&before, HOOK, marker()).unwrap();
        let text = serialize_settings(&installed);
        let zeta = text.find("zeta").unwrap();
        let alpha = text.find("alpha").unwrap();
        assert!(zeta < alpha, "keys were reordered:\n{text}");
    }

    /// An event key we emptied is removed; one that arrived empty is kept.
    /// Both halves matter: the first stops uninstall leaving `"Stop": []`
    /// residue, the second respects a user who keeps a deliberately empty key.
    #[test]
    fn remove_prunes_keys_it_emptied_but_keeps_ones_that_were_already_empty() {
        let cmd = hook_command(HOOK, marker());
        let before = json!({
            "hooks": {
                "Stop": [{"hooks": [{"type": "command", "command": cmd}]}],
                "Notification": []
            }
        });
        let cleaned = remove_marker_entries(&before, marker());
        let hooks = cleaned["hooks"].as_object().unwrap();
        assert!(
            !hooks.contains_key("Stop"),
            "a key we emptied must be pruned, not left as []"
        );
        assert_eq!(
            hooks.get("Notification"),
            Some(&json!([])),
            "a key that was already empty is the user's, and stays"
        );
    }

    #[test]
    fn remove_is_a_no_op_when_nothing_of_ours_is_present() {
        let before = other_consumers();
        assert_eq!(remove_marker_entries(&before, marker()), before);
    }

    #[test]
    fn add_works_against_a_completely_empty_settings_file() {
        let after = add_hook_entries(&json!({}), HOOK, marker()).unwrap();
        for event in HOOK_EVENTS {
            assert_eq!(
                after["hooks"][event][0]["hooks"][0]["command"],
                json!(hook_command(HOOK, marker()))
            );
            assert_eq!(
                after["hooks"][event][0]["hooks"][0]["type"],
                json!("command")
            );
        }
    }

    #[test]
    fn serialization_is_two_space_indented_with_a_trailing_newline() {
        let text = serialize_settings(&json!({"a": {"b": 1}}));
        assert_eq!(text, "{\n  \"a\": {\n    \"b\": 1\n  }\n}\n");
    }
}
