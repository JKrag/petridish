//! Reads Claude subscription quota from Claude Code's own `last-status.json`.
//! Mirrors `src/petridish/sensors/quota.py`. Account-global (not per-project) — feeds
//! `Radar.quota`, never a `Project`.

use crate::schema::QuotaState;
use chrono::{DateTime, TimeZone, Utc};
use std::path::Path;

/// Beyond this horizon, a reported `resets_at` is treated as garbage rather than trusted.
/// Both windows are at most 7 days, so a reset more than 30 days out means we misread the
/// units (seconds vs milliseconds, say) and showing "resets in 20000d" would be worse than
/// showing nothing.
const MAX_RESET_HORIZON_S: i64 = 30 * 24 * 3600;

/// Reads and parses the JSON file at `path`. Returns `None` (never panics/errors) on:
/// missing file, unreadable file, malformed JSON, a non-mapping top-level payload, or
/// out-of-range/boolean percentage fields. Partial payloads degrade field-by-field — a
/// dropped window key or renamed/mis-nested field leaves the rest of the struct intact
/// rather than failing the whole parse.
///
/// A naive (no-tz) timestamp is assumed UTC. An implausible reset timestamp (more than a
/// 30-day horizon out) is dropped (`None`) rather than trusted.
pub fn read_quota(path: &Path) -> Option<QuotaState> {
    let text = std::fs::read_to_string(path).ok()?;
    let payload: serde_json::Value = serde_json::from_str(&text).ok()?;
    let now = Utc::now();
    parse_value(&payload, now)
}

/// Build a `QuotaState` from a decoded value. Mirrors `parse_status` in the Python
/// reference, field-by-field tolerant of any shape change — every individual field is
/// optional and degrades independently. Returns `None` only when the top-level payload
/// is not a JSON object; partial truth beats absence.
fn parse_value(payload: &serde_json::Value, now: DateTime<Utc>) -> Option<QuotaState> {
    let obj = match payload.as_object() {
        Some(o) => o,
        None => return None,
    };

    let state = QuotaState {
        measured_at: parse_ts(obj.get("ts"), now),
        five_hour_used_pct: pct(obj.get("rate_limits").and_then(|v| v.get("five_hour")).and_then(|v| v.get("used_percentage"))),
        five_hour_resets_at: epoch_to_dt(
            obj.get("rate_limits")
                .and_then(|v| v.get("five_hour"))
                .and_then(|v| v.get("resets_at")),
            now,
        ),
        seven_day_used_pct: pct(obj.get("rate_limits").and_then(|v| v.get("seven_day")).and_then(|v| v.get("used_percentage"))),
        seven_day_resets_at: epoch_to_dt(
            obj.get("rate_limits")
                .and_then(|v| v.get("seven_day"))
                .and_then(|v| v.get("resets_at")),
            now,
        ),
        context_used_pct: pct(obj.get("context_window").and_then(|v| v.get("used_percentage"))),
    };

    // Nothing recognisable -> None so a header can omit the line entirely. `QuotaState`
    // doesn't derive `Default` (it's the schema's wire-contract struct, kept minimal), so
    // check all-None directly rather than adding a trait impl this module doesn't own.
    let all_none = state.measured_at.is_none()
        && state.five_hour_used_pct.is_none()
        && state.five_hour_resets_at.is_none()
        && state.seven_day_used_pct.is_none()
        && state.seven_day_resets_at.is_none()
        && state.context_used_pct.is_none();
    if all_none {
        return None;
    }
    Some(state)
}

/// Parse `used_percentage`: must be an integer 0-100. A boolean, a string, or an
/// out-of-range number degrades to `None`. Note: JSON booleans are decoded as `Value::Bool`
/// by `serde_json`, so an explicit `bool` check is still redundant — but kept because it
/// documents the invariant that *no* boolean-cast can leak through (independent of the
/// horizon guard).
fn pct(value: Option<&serde_json::Value>) -> Option<u8> {
    let v = value?;
    if v.is_boolean() {
        return None;
    }
    let n = match v.as_i64() {
        Some(n) => n,
        None => return None,
    };
    if !(0..=100).contains(&n) {
        return None;
    }
    Some(n as u8)
}

/// Convert an epoch-seconds number (int or float, not bool) to UTC DateTime.
/// Rejects: booleans explicitly (it would otherwise become 1970-01-01), non-numbers,
/// overflows on `from_timestamp`, and timestamps more than 30 days away in either
/// direction from `now` (implausible garbage).
fn epoch_to_dt(value: Option<&serde_json::Value>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let v = value?;
    if v.is_boolean() {
        return None;
    }
    let n = match *v {
        serde_json::Value::Number(ref n) => n.as_f64(),
        _ => None,
    };
    let n = match n {
        Some(f) if f.is_finite() => f,
        _ => return None,
    };
    let secs = n as i64;
    let subsec = ((n - secs as f64) * 1_000_000_000_f64).round() as u32;
    let dt = match DateTime::from_timestamp(secs, subsec) {
        Some(dt) => dt,
        None => return None,
    };
    if (dt - now).num_seconds().unsigned_abs() > MAX_RESET_HORIZON_S as u64 {
        return None;
    }
    Some(dt)
}

/// Parse Claude Code's own `ts` field — an ISO-8601 timestamp, treated as UTC (a naive
/// value is assumed UTC). An out-of-range or malformed value degrades to `None`. Same
/// 30-day plausibility check as `epoch_to_dt`, since the same units-misread risk applies.
fn parse_ts(value: Option<&serde_json::Value>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let s = value?.as_str()?;
    // `fromisoformat` in Python is forgiving of the trailing `Z`; Rust's
    // `chrono` `DateTime::parse_from_rfc3339` requires either offset or `Z`. Handle both.
    let dt = match DateTime::parse_from_rfc3339(s) {
        Ok(dt) => dt.with_timezone(&Utc),
        Err(_) => {
            // Fallback: naive value like "2026-08-09T06:32:11" (no offset, no Z). Treat as UTC.
            match chrono::NaiveDateTime::parse_from_str(s.trim_end_matches('Z'), "%Y-%m-%dT%H:%M:%S")
            {
                Ok(ndt) => Utc.from_utc_datetime(&ndt),
                Err(_) => return None,
            }
        }
    };
    if (dt - now).num_seconds().unsigned_abs() > MAX_RESET_HORIZON_S as u64 {
        return None;
    }
    Some(dt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // Helper: write a JSON file at a random tmp path, return its path for later reads.
    struct Tmp {
        path: std::path::PathBuf,
    }
    impl Tmp {
        fn new(suffix: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("swab_rs_quota_test_{suffix}"));
            let _ = std::fs::remove_file(&path);
            Self { path }
        }
    }

    fn write_json(path: &Path, value: &serde_json::Value) {
        let mut fh = std::fs::File::create(path).expect("write");
        let text = serde_json::to_string(value).expect("serialize");
        fh.write_all(text.as_bytes()).expect("write bytes");
    }

    // Test 1: missing file -> None.
    #[test]
    fn missing_file_returns_none() {
        let result = read_quota(std::path::Path::new(
            "/definitely/does/not/exist/last-status.json",
        ));
        assert!(result.is_none(), "missing file must return None");
    }

    // Test 2: full valid payload -> all six QuotaState fields populated.
    #[test]
    fn full_valid_payload_populates_all_fields() {
        // Use timestamps we can predict relative to "now".
        let now = Utc::now();
        let five_hour_ts_str = "2026-08-09T06:32:11Z";
        // Pick a resets_at within the horizon window, relative to now.
        let five_hour_resets = (now + chrono::Duration::hours(2)).timestamp();
        let seven_day_resets = (now + chrono::Duration::hours(48)).timestamp();

        let payload = serde_json::json!({
            "ts": five_hour_ts_str,
            "rate_limits": {
                "five_hour": { "used_percentage": 9, "resets_at": five_hour_resets },
                "seven_day": { "used_percentage": 86, "resets_at": seven_day_resets }
            },
            "context_window": { "used_percentage": 28 }
        });

        let result = parse_value(&payload, now);
        assert!(result.is_some(), "full valid payload must succeed");

        let state = result.unwrap();
        assert_eq!(state.five_hour_used_pct, Some(9));
        assert_eq!(state.seven_day_used_pct, Some(86));
        assert_eq!(state.context_used_pct, Some(28));
        assert!(state.five_hour_resets_at.is_some(), "five_hour resets_at must be set");
        assert!(state.seven_day_resets_at.is_some(), "seven_day resets_at must be set");
        assert!(state.measured_at.is_some(), "measured_at must be set");
    }

    // Test 3: malformed JSON -> None.
    #[test]
    fn malformed_json_returns_none() {
        let tmp = Tmp::new("malformed");
        std::fs::write(&tmp.path, "NOT JSON").expect("write");

        let result = read_quota(&tmp.path);
        assert!(result.is_none(), "malformed JSON must return None");
    }

    // Test 4: rate_limits entirely absent -> None for the two rate-limit-derived pairs,
    // but context_used_pct still populates if context_window is present and valid.
    #[test]
    fn missing_rate_limits_but_present_context_window() {
        let payload = serde_json::json!({
            "context_window": { "used_percentage": 42 }
        });
        let now = Utc::now();
        let result = parse_value(&payload, now);

        let state = result.expect("must succeed since context_window is valid");
        assert_eq!(state.five_hour_used_pct, None);
        assert_eq!(state.five_hour_resets_at, None);
        assert_eq!(state.seven_day_used_pct, None);
        assert_eq!(state.seven_day_resets_at, None);
        assert_eq!(state.context_used_pct, Some(42));
        // measured_at is None since `ts` was absent.
        assert_eq!(state.measured_at, None);
    }

    // Test 5: used_percentage: true (a boolean) -> that specific field is None,
    // the rest of the struct still populates from valid sibling fields.
    #[test]
    fn bool_percentage_is_none_rest_parses() {
        let payload = serde_json::json!({
            "ts": "2026-08-09T06:32:11Z",
            "rate_limits": {
                "five_hour": { "used_percentage": true, "resets_at": 1786275000 },
                "seven_day": { "used_percentage": 86, "resets_at": 1786431600 }
            },
            "context_window": { "used_percentage": 28 }
        });

        // Use a now that's close to the fixed resets_at timestamps (so they pass plausibility).
        let fixed_dt = chrono::NaiveDateTime::parse_from_str("2026-06-08T12:50:00", "%Y-%m-%dT%H:%M:%S")
            .unwrap()
            .and_utc();
        let now = fixed_dt;

        let result = parse_value(&payload, now).expect("still must succeed since other fields valid");

        assert_eq!(result.five_hour_used_pct, None, "boolean must be dropped");
        assert_eq!(result.seven_day_used_pct, Some(86));
        assert_eq!(result.context_used_pct, Some(28));
    }

    // Test 6: used_percentage: 150 (out of 0-100) -> None for that field. A companion
    // valid field (five_hour) is required so `state` isn't ALL-None -- an invalid value
    // is equivalent to absence for the "nothing recognisable -> None" check, matching
    // the real Python test (`test_out_of_range_or_non_numeric_percentages_are_dropped`,
    // which always keeps `context_window.used_percentage: 28` alongside the bad field).
    #[test]
    fn out_of_range_percentage_is_none() {
        let payload = serde_json::json!({
            "rate_limits": { "five_hour": { "used_percentage": 9 } },
            "context_window": { "used_percentage": 150 }
        });
        let now = Utc::now();
        let result = parse_value(&payload, now).expect("must succeed since five_hour is valid");
        assert_eq!(result.context_used_pct, None, "150 must be dropped");
        assert_eq!(result.five_hour_used_pct, Some(9));
    }

    // Test 7: resets_at more than 30 days out from `now` -> field is None.
    #[test]
    fn resets_at_beyond_30_days_is_none() {
        let payload = serde_json::json!({
            "rate_limits": {
                "five_hour": { "used_percentage": 9, "resets_at": 9_999_000_000i64 }
            },
            "context_window": { "used_percentage": 28 }
        });
        let now = Utc::now();
        let result = parse_value(&payload, now).expect("still succeeds because context_window valid");

        assert_eq!(result.five_hour_resets_at, None, "too-far-in-future reset must be None");
        assert_eq!(result.five_hour_used_pct, Some(9));
        assert_eq!(result.context_used_pct, Some(28));
    }

    // Test 8: non-mapping top-level JSON (e.g. a bare array) -> None overall.
    #[test]
    fn non_object_top_level_returns_none() {
        let payload = serde_json::json!([1, 2, 3]);
        let result = parse_value(&payload, Utc::now());
        assert!(result.is_none(), "array top-level must return None");
    }

    // Test: empty payload -> None (nothing recognisable).
    #[test]
    fn empty_object_returns_none() {
        let payload = serde_json::json!({});
        let result = parse_value(&payload, Utc::now());
        assert!(result.is_none(), "empty object must return None");
    }

    // Test: string used_percentage is None. Companion valid field for the same reason as
    // `out_of_range_percentage_is_none` above.
    #[test]
    fn string_used_percentage_is_none() {
        let payload = serde_json::json!({
            "rate_limits": { "five_hour": { "used_percentage": 9 } },
            "context_window": { "used_percentage": "28" }
        });
        let now = Utc::now();
        let result = parse_value(&payload, now).expect("still succeeds because five_hour is valid");
        assert_eq!(result.context_used_pct, None);
    }

    // Test: negative used_percentage is None.
    #[test]
    fn negative_used_percentage_is_none() {
        assert_eq!(pct(Some(&serde_json::json!(-5))), None);
    }

    // Test: resets_at as a string is None.
    #[test]
    fn resets_at_string_is_none() {
        let payload = serde_json::json!({
            "rate_limits": {
                "five_hour": { "used_percentage": 9, "resets_at": "not-an-integer" }
            },
            "context_window": { "used_percentage": 28 }
        });
        let now = Utc::now();
        let result = parse_value(&payload, now).expect("still succeeds");
        assert_eq!(result.five_hour_resets_at, None);
        assert_eq!(result.five_hour_used_pct, Some(9));
    }

    // Test: measured_at with a string that parses cleanly.
    #[test]
    fn measured_at_parses_rfc3339() {
        let now = Utc::now();
        let result = parse_ts(Some(&serde_json::json!("2026-08-09T06:32:11Z")), now);
        assert!(result.is_some(), "valid ISO-8601 string must parse");
    }

    // Test: measured_at with a non-string is None.
    #[test]
    fn measured_at_non_string_is_none() {
        let now = Utc::now();
        let result = parse_ts(Some(&serde_json::json!(42)), now);
        assert!(result.is_none());
    }

    // Test: measured_at far in the future -> None (plausibility).
    #[test]
    fn measured_at_far_future_is_none() {
        let now = Utc::now();
        // 2030 is more than 30 days away.
        let result = parse_ts(Some(&serde_json::json!("2030-01-01T00:00:00Z")), now);
        assert!(result.is_none(), "2030 must fail the 30-day plausibility check");
    }

    // Test: a resets_at in the distant past (e.g. 1970-01-01) is None.
    #[test]
    fn resets_at_1970_is_none() {
        let payload = serde_json::json!({
            "rate_limits": {
                "five_hour": { "used_percentage": 9, "resets_at": 0 }
            },
            "context_window": { "used_percentage": 28 }
        });
        let now = Utc::now();
        let result = parse_value(&payload, now).expect("still succeeds");
        assert_eq!(result.five_hour_resets_at, None, "epoch 0 (1970) is more than 30 days away from now");
    }

    // Test: bool `used_percentage` on five_hour — that field None but rest parses.
    #[test]
    fn bool_five_hour_used_percentage() {
        let payload = serde_json::json!({
            "rate_limits": {
                "five_hour": { "used_percentage": false, "resets_at": 1786275000 },
                "seven_day": { "used_percentage": 86, "resets_at": 1786431600 }
            },
            "context_window": { "used_percentage": 28 }
        });
        // Use a now that's close to the fixed resets_at timestamps.
        let fixed_dt = chrono::NaiveDateTime::parse_from_str("2026-06-08T12:50:00", "%Y-%m-%dT%H:%M:%S")
            .unwrap()
            .and_utc();
        let result = parse_value(&payload, fixed_dt).expect("other fields must still parse");
        assert_eq!(result.five_hour_used_pct, None);
        assert_eq!(result.seven_day_used_pct, Some(86));
    }

    // Test: a resets_at that's exactly at the 30-day boundary (inclusive) is accepted,
    // and one just past is rejected.
    #[test]
    fn resets_at_exactly_30_days_in_past_is_accepted() {
        let now = Utc::now();
        let thirty_days_ago = (now - chrono::Duration::days(30)).timestamp();
        // The horizon check: |dt - now| > MAX_RESET_HORIZON_S -> drop. We need (now - thirty_days_ago)
        // == 30 days, which is NOT greater than MAX_RESET_HORIZON_S. So this should parse.
        let payload = serde_json::json!({
            "rate_limits": {
                "five_hour": { "used_percentage": 9, "resets_at": thirty_days_ago }
            },
            "context_window": { "used_percentage": 28 }
        });
        let result = parse_value(&payload, now).expect("exactly-30-day resets_at must be accepted");
        assert!(result.five_hour_resets_at.is_some());

        // 31 days ago -> drop.
        let thirty_one_days_ago = (now - chrono::Duration::days(31)).timestamp();
        let payload2 = serde_json::json!({
            "rate_limits": {
                "five_hour": { "used_percentage": 9, "resets_at": thirty_one_days_ago }
            },
            "context_window": { "used_percentage": 28 }
        });
        let result2 = parse_value(&payload2, now).expect("other fields must still parse");
        assert_eq!(result2.five_hour_resets_at, None);
    }

    // Test: context_window is entirely absent -> context_used_pct is None but other fields still parse.
    #[test]
    fn missing_context_window_is_none_but_others_parse() {
        let payload = serde_json::json!({
            "rate_limits": {
                "five_hour": { "used_percentage": 9, "resets_at": 1786275000 },
                "seven_day": { "used_percentage": 86, "resets_at": 1786431600 }
            }
        });
        let fixed_dt = chrono::NaiveDateTime::parse_from_str("2026-06-08T12:50:00", "%Y-%m-%dT%H:%M:%S")
            .unwrap()
            .and_utc();
        let result = parse_value(&payload, fixed_dt).expect("must still succeed without context_window");
        assert_eq!(result.context_used_pct, None);
        assert_eq!(result.five_hour_used_pct, Some(9));
    }

    // Test: full parse via read_quota on a real file.
    #[test]
    fn read_quota_full_round_trip() {
        let tmp = Tmp::new("roundtrip");

        let payload = serde_json::json!({
            "ts": "2026-06-08T12:50:00Z",
            "rate_limits": {
                "five_hour": { "used_percentage": 9, "resets_at": 1786275000 },
                "seven_day": { "used_percentage": 86, "resets_at": 1786431600 }
            },
            "context_window": { "used_percentage": 28 }
        });
        write_json(&tmp.path, &payload);

        let result = read_quota(&tmp.path);
        assert!(result.is_some(), "must succeed on a real file");
        let s = result.unwrap();
        assert_eq!(s.five_hour_used_pct, Some(9));
        assert_eq!(s.seven_day_used_pct, Some(86));
        assert_eq!(s.context_used_pct, Some(28));
    }

    // Test: top-level string -> None.
    #[test]
    fn top_level_string_returns_none() {
        let payload = serde_json::json!("hello");
        let result = parse_value(&payload, Utc::now());
        assert!(result.is_none());
    }

    // Test: top-level number -> None.
    #[test]
    fn top_level_number_returns_none() {
        let payload = serde_json::json!(42);
        let result = parse_value(&payload, Utc::now());
        assert!(result.is_none());
    }

    // Test: top-level null -> None.
    #[test]
    fn top_level_null_returns_none() {
        let payload = serde_json::json!(null);
        let result = parse_value(&payload, Utc::now());
        assert!(result.is_none());
    }

    // Test: empty rate_limits object -> both five_hour and seven_day are None (other fields still parse).
    #[test]
    fn empty_rate_limits_object() {
        let payload = serde_json::json!({
            "context_window": { "used_percentage": 28 }
        });
        let now = Utc::now();
        let result = parse_value(&payload, now).expect("context_window must still work");
        assert_eq!(result.five_hour_used_pct, None);
        assert_eq!(result.seven_day_used_pct, None);
        assert_eq!(result.context_used_pct, Some(28));
    }

    // Test: rate_limits key is null -> both five_hour and seven_day are None.
    #[test]
    fn null_rate_limits() {
        let payload = serde_json::json!({
            "rate_limits": null,
            "context_window": { "used_percentage": 28 }
        });
        let now = Utc::now();
        let result = parse_value(&payload, now).expect("must succeed");
        assert_eq!(result.five_hour_used_pct, None);
        assert_eq!(result.seven_day_used_pct, None);
    }

    // Test: context_window is null -> context_used_pct is None.
    #[test]
    fn null_context_window() {
        let payload = serde_json::json!({
            "context_window": null,
            "rate_limits": {
                "five_hour": { "used_percentage": 9, "resets_at": 1786275000 },
                "seven_day": { "used_percentage": 86, "resets_at": 1786431600 }
            }
        });
        let fixed_dt = chrono::NaiveDateTime::parse_from_str("2026-06-08T12:50:00", "%Y-%m-%dT%H:%M:%S")
            .unwrap()
            .and_utc();
        let result = parse_value(&payload, fixed_dt).expect("must succeed");
        assert_eq!(result.context_used_pct, None);
        assert_eq!(result.five_hour_used_pct, Some(9));
    }
}
