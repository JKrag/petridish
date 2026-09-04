//! S2 acceptance gate (petri/SPEC.md §8/§9, ADR-0003) — protected, authored by the
//! orchestrator, not the delegate. Deserializes all four committed fixtures into
//! `Radar` and asserts shape/counts. This is deliberately loose on content (that is
//! what the layer-2 snapshot tests in S4+ are for) and strict on structure: every
//! fixture must parse, and every documented trait from petri/SPEC.md §8's fixture
//! table must be structurally present somewhere in the file.
//!
//! Note on `hostile.json`'s "empty project list · all-cold" bullet (ADR-0003): read
//! literally, "empty project list" and the rest of that fixture's per-project traits
//! (null branch, 200-char name, worktree parent absent, ...) are mutually exclusive —
//! an empty `projects: []` cannot also contain a 200-char-name project. The only
//! internally-consistent reading, given every other bullet in that list names a
//! concrete per-project trait, is that "empty" describes the *bucket* content, not
//! the array: every project in `hostile.json` sits in the `cold` bucket (which
//! "all-cold" already says), i.e. the fixture represents a dashboard with nothing
//! running. This test enforces that reading (`hostile_all_projects_are_cold`) rather
//! than a literal empty list. Flagging this here per CLAUDE.md's shortcut-disclosure
//! rule — if that reading is wrong, this is the one place to fix it.
//!
//! Was `#[ignore]`d while the fixtures didn't exist yet (so `cargo test --workspace`
//! / `make check` / CI stayed green pending S2); stripped now that S2 landed
//! (`.afk/verify-petri-rust.sh` ran this file with `--ignored` for the S2 gate
//! itself, all 12 passing). Runs as an ordinary part of `cargo test --workspace`
//! from here on.

use petridish_core::schema::{Radar, StatusBucket};
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join(name)
}

fn load(name: &str) -> Radar {
    let text = std::fs::read_to_string(fixture_path(name))
        .unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("fixture {name} failed to deserialize into Radar: {e}"))
}

#[test]
fn minimal_has_exactly_one_project() {
    let radar = load("minimal.json");
    assert_eq!(
        radar.projects.len(),
        1,
        "minimal.json must contain exactly one project"
    );
}

#[test]
fn normal_has_roughly_fifteen_projects_in_mixed_buckets() {
    let radar = load("normal.json");
    let n = radar.projects.len();
    assert!(
        (12..=18).contains(&n),
        "normal.json should have ~15 projects (12..=18), got {n}"
    );
    let distinct_bucket_count = [
        StatusBucket::Active,
        StatusBucket::InFlight,
        StatusBucket::Stale,
        StatusBucket::Cold,
    ]
    .into_iter()
    .filter(|b| radar.projects.iter().any(|p| p.status_bucket == *b))
    .count();
    assert!(
        distinct_bucket_count >= 2,
        "normal.json must mix buckets, got only {distinct_bucket_count} distinct bucket(s)"
    );
}

#[test]
fn loaded_has_roughly_seventy_projects_every_bucket_and_a_worktree() {
    let radar = load("loaded.json");
    let n = radar.projects.len();
    assert!(
        (60..=80).contains(&n),
        "loaded.json should have ~70 projects (60..=80), got {n}"
    );
    for bucket in [
        StatusBucket::Active,
        StatusBucket::InFlight,
        StatusBucket::Stale,
        StatusBucket::Cold,
    ] {
        assert!(
            radar.projects.iter().any(|p| p.status_bucket == bucket),
            "loaded.json must populate every StatusBucket, missing {bucket:?}"
        );
    }
    assert!(
        radar.projects.iter().any(|p| p.parent_path.is_some()),
        "loaded.json must contain at least one worktree project (parent_path set)"
    );
}

#[test]
fn hostile_deserializes_and_has_a_far_future_schema_version() {
    let radar = load("hostile.json");
    assert!(
        radar.schema_version > 1,
        "hostile.json's schema_version must be from the future (> the current known version 1), got {}",
        radar.schema_version
    );
}

#[test]
fn hostile_updated_at_is_stale() {
    let radar = load("hostile.json");
    let age = chrono::Utc::now().signed_duration_since(radar.updated_at);
    assert!(
        age > chrono::Duration::hours(24),
        "hostile.json's updated_at must be old enough to trip the staleness banner (>24h), age was {age}"
    );
}

#[test]
fn hostile_quota_is_absent() {
    let radar = load("hostile.json");
    assert!(
        radar.quota.is_none(),
        "hostile.json must have absent quota (quota: null)"
    );
}

#[test]
fn hostile_all_projects_are_cold() {
    // See the module doc comment above for why this, not a literal empty `projects: []`.
    let radar = load("hostile.json");
    assert!(
        !radar.projects.is_empty(),
        "hostile.json must contain projects to exercise per-project traits"
    );
    assert!(
        radar
            .projects
            .iter()
            .all(|p| p.status_bucket == StatusBucket::Cold),
        "hostile.json must be all-cold — every project's status_bucket must be Cold"
    );
}

#[test]
fn hostile_has_a_non_repo_project() {
    let radar = load("hostile.json");
    assert!(
        radar.projects.iter().any(|p| !p.git.is_repo),
        "hostile.json must contain a non-repo project (git.is_repo == false)"
    );
}

#[test]
fn hostile_has_a_null_branch_project() {
    let radar = load("hostile.json");
    assert!(
        radar.projects.iter().any(|p| p.git.branch.is_none()),
        "hostile.json must contain a project with a null branch"
    );
}

#[test]
fn hostile_has_a_two_hundred_char_name() {
    let radar = load("hostile.json");
    assert!(
        radar.projects.iter().any(|p| p.name.chars().count() == 200),
        "hostile.json must contain a project whose name is exactly 200 characters"
    );
}

#[test]
fn hostile_has_a_cjk_or_emoji_name() {
    let radar = load("hostile.json");
    assert!(
        radar
            .projects
            .iter()
            .any(|p| p.name.chars().any(|c| c as u32 > 0x2E80)),
        "hostile.json must contain a project with a CJK or emoji name (a codepoint above U+2E80)"
    );
}

#[test]
fn hostile_has_a_worktree_with_an_absent_parent() {
    let radar = load("hostile.json");
    let all_paths: std::collections::HashSet<&str> =
        radar.projects.iter().map(|p| p.path.as_str()).collect();
    assert!(
        radar.projects.iter().any(|p| p
            .parent_path
            .as_deref()
            .is_some_and(|pp| !all_paths.contains(pp))),
        "hostile.json must contain a worktree project whose parent_path does not match any project's own path"
    );
}
