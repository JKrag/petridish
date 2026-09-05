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

use crate::error::InstallError;
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
///
/// The path is **shell-quoted**. Claude Code hands this string to a shell, and
/// an unquoted `/Volumes/Dev Disk/bin/swab-hook` makes it try to run
/// `/Volumes/Dev`. The Python original had the same defect; it was faithfully
/// ported and is fixed here.
///
/// Quoting does not affect identification: `has_marker` and friends look for the
/// marker substring, which is appended outside the quotes either way, so an
/// entry written by an older version is still recognised as ours.
pub fn hook_command(hook_abspath: &str, marker: &str) -> String {
    format!("{} {marker}", crate::shell::single_quote(hook_abspath))
}

/// Add or refresh our hook group under every event in `HOOK_EVENTS`.
///
/// Returns `None` when nothing needed changing, so the caller can say "left
/// untouched" rather than rewriting an unchanged file.
///
/// **Idempotent per event, not per file.** The original check short-circuited on
/// the marker appearing anywhere, so a user who installed before
/// `Notification`/`PermissionRequest` joined `HOOK_EVENTS` would re-run
/// `install`, be told "already installed", and silently never receive the two
/// new events — leaving the "waiting on you" feature live in the code and dead
/// on the only machine that had ever installed it.
///
/// **It also refreshes a stale command.** A marked group whose command no longer
/// matches the resolved binary path is *replaced*, not left alone. Without this,
/// re-running `install` after an upgrade relocated `swab-hook` — which the README
/// documents as the fix — would rewrite the launchd plist and leave every hook
/// entry pointing at a path that no longer exists, while `doctor` reported
/// success because the marker was still present.
///
/// Errors rather than guessing when `settings.json` has a shape we do not
/// recognise: a non-object root, or a `hooks` value that is not an object.
/// Silently replacing either would discard data this tool does not own, which is
/// exactly what D4 forbids.
pub fn add_hook_entries(
    settings: &Value,
    hook_abspath: &str,
    marker: &str,
) -> Result<Option<Value>, InstallError> {
    let Some(root) = settings.as_object() else {
        return Err(InstallError::UnexpectedSettingsShape(
            "the root of settings.json is not a JSON object".into(),
        ));
    };
    let hooks_value = root.get("hooks");
    let hooks_map = match hooks_value {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => {
            return Err(InstallError::UnexpectedSettingsShape(
                "the \"hooks\" key in settings.json is not a JSON object".into(),
            ));
        }
    };

    let command = hook_command(hook_abspath, marker);
    let mut hooks = hooks_map;
    let mut changed = false;

    for event in HOOK_EVENTS {
        let existing = hooks.get(event);
        let mut groups = match existing {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(a)) => a.clone(),
            Some(_) => {
                return Err(InstallError::UnexpectedSettingsShape(format!(
                    "hooks.{event} in settings.json is not an array"
                )));
            }
        };

        let ours: Vec<usize> = groups
            .iter()
            .enumerate()
            .filter(|(_, g)| is_marked_group(g, marker))
            .map(|(i, _)| i)
            .collect();

        if ours.is_empty() {
            groups.push(new_group(&command));
            changed = true;
        } else {
            // Replace every one of ours whose command has drifted, and collapse
            // any accidental duplicates down to the first.
            let keep = ours[0];
            let wanted = new_group(&command);
            if groups[keep] != wanted {
                groups[keep] = wanted;
                changed = true;
            }
            for &i in ours.iter().skip(1).rev() {
                groups.remove(i);
                changed = true;
            }
        }

        hooks.insert(event.to_string(), Value::Array(groups));
    }

    if !changed {
        return Ok(None);
    }

    let mut updated = root.clone();
    updated.insert("hooks".into(), Value::Object(hooks));
    Ok(Some(Value::Object(updated)))
}

/// One hook group in the shape Claude Code expects, and the shape
/// `remove_marker_entries` knows how to take back out again.
fn new_group(command: &str) -> Value {
    let mut entry = Map::new();
    entry.insert("type".into(), Value::String("command".into()));
    entry.insert("command".into(), Value::String(command.to_string()));
    let mut group = Map::new();
    group.insert("hooks".into(), Value::Array(vec![Value::Object(entry)]));
    Value::Object(group)
}

/// Is `item` a *dict* whose subtree carries the marker? Only dict elements of a
/// list are ever dropped — exactly the shape `add_hook_entries` adds.
fn is_marked_group(item: &Value, marker: &str) -> bool {
    item.is_object() && contains_marker(item, marker)
}

/// Remove every hook group we added, and nothing else.
///
/// **Scoped to `hooks.<event>` arrays only.** An earlier version walked the
/// entire document and dropped any object, in any array, whose subtree contained
/// the marker — so an unrelated entry that merely happened to mention
/// `# petridish` (a note in a `permissions` list, say) would have been deleted by
/// `uninstall`. That is precisely the promise D4 makes and it was not being kept.
/// The Python original had the same over-broad behaviour.
///
/// Sibling entries from other consumers, and every key we did not write, pass
/// through unchanged — which is why uninstall must never restore from the
/// backup: the backup predates any edits those consumers made since.
///
/// An event key we were the *sole* consumer of is dropped along with its last
/// group, so uninstall leaves no `"PermissionRequest": []` residue. The exception
/// matters: a key that was **already** empty before the drop is kept, because
/// that is a deliberate choice by the user and not something we emptied.
pub fn remove_marker_entries(settings: &Value, marker: &str) -> Value {
    let Some(root) = settings.as_object() else {
        return settings.clone();
    };
    let Some(before) = root.get("hooks").and_then(Value::as_object) else {
        return settings.clone();
    };

    let mut pruned: Map<String, Value> = Map::new();
    for (event, groups) in before {
        let Some(list) = groups.as_array() else {
            // Not a shape we wrote; leave it exactly as found.
            pruned.insert(event.clone(), groups.clone());
            continue;
        };
        let was_empty_before = list.is_empty();
        let kept: Vec<Value> = list
            .iter()
            .filter(|g| !is_marked_group(g, marker))
            .cloned()
            .collect();

        // Drop a key we emptied; keep one that arrived empty.
        if kept.is_empty() && !was_empty_before {
            continue;
        }
        pruned.insert(event.clone(), Value::Array(kept));
    }

    let mut out = root.clone();
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
        let after = add_hook_entries(&before, HOOK, marker())
            .unwrap()
            .expect("must change");

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
        let once = add_hook_entries(&other_consumers(), HOOK, marker())
            .unwrap()
            .unwrap();
        assert!(
            add_hook_entries(&once, HOOK, marker()).unwrap().is_none(),
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
            .unwrap()
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
        let installed = add_hook_entries(&before, HOOK, marker()).unwrap().unwrap();
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
        let installed = add_hook_entries(&before, HOOK, marker()).unwrap().unwrap();
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
        let installed = add_hook_entries(&before, HOOK, marker()).unwrap().unwrap();
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
        let after = add_hook_entries(&json!({}), HOOK, marker())
            .unwrap()
            .unwrap();
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

    // ── Findings from review of PR #5 ────────────────────────────────────

    /// Claude Code runs this string through a shell. Unquoted,
    /// `/Volumes/Dev Disk/bin/swab-hook` becomes an attempt to run
    /// `/Volumes/Dev`.
    #[test]
    fn the_hook_command_is_shell_quoted_so_a_spaced_path_stays_one_word() {
        let cmd = hook_command("/Volumes/Dev Disk/bin/swab-hook", marker());
        assert_eq!(cmd, "'/Volumes/Dev Disk/bin/swab-hook' # petridish");
        assert!(
            cmd.contains(marker()),
            "the marker must stay outside the quotes"
        );
    }

    /// Quoting changed the command text, so confirm identification still works —
    /// an entry written by an older, unquoted version must still be recognised.
    #[test]
    fn an_unquoted_legacy_entry_is_still_recognised_as_ours() {
        let legacy = json!({
            "hooks": {"Stop": [{"hooks": [{"type": "command",
                "command": "/Users/x/.cargo/bin/swab-hook # petridish"}]}]}
        });
        assert!(has_marker(&legacy, marker()));
        assert!(event_has_marker(&legacy, "Stop", marker()));
    }

    /// The documented recovery after an upgrade relocates the binaries is
    /// "re-run `petridish install`". That only works if a marked entry whose
    /// path has gone stale is actually replaced — otherwise the plist is
    /// rewritten, every hook still points at a path that no longer exists, and
    /// `doctor` reports success because the marker is present.
    #[test]
    fn reinstalling_after_a_move_refreshes_the_stale_hook_path() {
        let old = add_hook_entries(&other_consumers(), "/old/bin/swab-hook", marker())
            .unwrap()
            .unwrap();
        let new = add_hook_entries(&old, "/opt/homebrew/bin/swab-hook", marker())
            .unwrap()
            .expect("a moved binary must be a change");

        for event in HOOK_EVENTS {
            let groups = new["hooks"][event].as_array().unwrap();
            let ours: Vec<&Value> = groups
                .iter()
                .filter(|g| contains_marker(g, marker()))
                .collect();
            assert_eq!(
                ours.len(),
                1,
                "{event}: exactly one of ours, not a duplicate"
            );
            let cmd = ours[0]["hooks"][0]["command"].as_str().unwrap();
            assert!(
                cmd.contains("/opt/homebrew/bin/swab-hook"),
                "{event}: {cmd}"
            );
            assert!(
                !cmd.contains("/old/bin"),
                "{event}: stale path survived: {cmd}"
            );
        }
        // Another consumer's entry is still untouched by the refresh.
        assert_eq!(
            new["hooks"]["PreToolUse"][0],
            other_consumers()["hooks"]["PreToolUse"][0]
        );
    }

    #[test]
    fn a_reinstall_with_an_unchanged_path_is_still_a_no_op() {
        let once = add_hook_entries(&other_consumers(), HOOK, marker())
            .unwrap()
            .unwrap();
        assert!(add_hook_entries(&once, HOOK, marker()).unwrap().is_none());
    }

    /// Silently normalising a shape we did not write is data loss, which is
    /// exactly what D4 forbids. Stop instead.
    #[test]
    fn an_unrecognised_settings_shape_is_an_error_not_a_silent_rewrite() {
        for bad in [json!([1, 2, 3]), json!("a string"), json!(42)] {
            assert!(
                add_hook_entries(&bad, HOOK, marker()).is_err(),
                "non-object root must be refused: {bad}"
            );
        }
        assert!(
            add_hook_entries(&json!({"hooks": "not an object"}), HOOK, marker()).is_err(),
            "a non-object `hooks` must be refused rather than discarded"
        );
        assert!(
            add_hook_entries(&json!({"hooks": {"Stop": "not an array"}}), HOOK, marker()).is_err(),
            "a non-array event must be refused rather than overwritten"
        );
        // A missing `hooks` key is normal, not an error.
        assert!(add_hook_entries(&json!({"model": "opus"}), HOOK, marker()).is_ok());
    }

    /// `uninstall` promises to remove only its own hook entries. Walking the
    /// whole document and dropping anything mentioning the marker broke that:
    /// an unrelated array entry that merely *mentions* petridish is not ours.
    #[test]
    fn uninstall_leaves_unrelated_arrays_alone_even_when_they_mention_the_marker() {
        let mut before = other_consumers();
        before["permissions"] = json!({
            "allow": [{"note": "allow the # petridish daemon to read repos"}]
        });
        let installed = add_hook_entries(&before, HOOK, marker()).unwrap().unwrap();
        let cleaned = remove_marker_entries(&installed, marker());

        assert_eq!(
            cleaned["permissions"], before["permissions"],
            "an unrelated array entry mentioning the marker must survive"
        );
        // ...while our actual hook groups are gone.
        for event in HOOK_EVENTS {
            assert!(!event_has_marker(&cleaned, event, marker()), "{event}");
        }
    }

    #[test]
    fn serialization_is_two_space_indented_with_a_trailing_newline() {
        let text = serialize_settings(&json!({"a": {"b": 1}}));
        assert_eq!(text, "{\n  \"a\": {\n    \"b\": 1\n  }\n}\n");
    }
}
