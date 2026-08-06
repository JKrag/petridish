import { test } from "node:test";
import assert from "node:assert/strict";
import { groupByBucket, filterProjects, isStale, agentLabel, bucketTitle } from "../src/lib/state.ts";
import type { Project, Radar } from "../src/types.ts";

function project(overrides: Partial<Project> = {}): Project {
  return {
    id: "p1",
    name: "sample",
    path: "/tmp/sample",
    category: "misc",
    is_foreign: false,
    git: {
      is_repo: true,
      branch: "main",
      is_dirty: false,
      uncommitted_files: 0,
      last_commit_at: null,
      mine_last_commit_at: null,
      github_url: null,
    },
    agent: {
      state: "idle",
      active_agent: null,
      last_event: null,
      last_event_at: null,
      session_id: null,
    },
    last_activity_at: null,
    status_bucket: "active",
    ...overrides,
  };
}

test("groupByBucket buckets by status_bucket and keeps all four keys", () => {
  const grouped = groupByBucket([
    project({ id: "a", status_bucket: "active" }),
    project({ id: "b", status_bucket: "cold" }),
  ]);
  assert.equal(grouped.active.length, 1);
  assert.equal(grouped.cold.length, 1);
  assert.equal(grouped.in_flight.length, 0);
  assert.equal(grouped.stale.length, 0);
});

test("groupByBucket excludes is_foreign projects", () => {
  const grouped = groupByBucket([project({ id: "a", is_foreign: true })]);
  assert.equal(grouped.active.length, 0);
});

test("filterProjects empty query returns everything", () => {
  const projects = [project({ id: "a", name: "alpha" }), project({ id: "b", name: "beta" })];
  assert.deepEqual(filterProjects(projects, ""), projects);
});

test("filterProjects matches case-insensitively and by substring", () => {
  const projects = [project({ id: "a", name: "Cat-Pedigree" }), project({ id: "b", name: "dog-walker" })];
  const result = filterProjects(projects, "cat");
  assert.equal(result.length, 1);
  assert.equal(result[0].id, "a");
});

test("filterProjects with no matches returns empty array", () => {
  const projects = [project({ id: "a", name: "alpha" })];
  assert.deepEqual(filterProjects(projects, "zzz"), []);
});

test("agentLabel includes agent name and state when active_agent is set", () => {
  const p = project({ agent: { state: "working", active_agent: "Claude Code", last_event: null, last_event_at: null, session_id: null } });
  assert.equal(agentLabel(p), "Claude Code (working)");
});

test("agentLabel falls back to bare state when no active_agent", () => {
  const p = project({ agent: { state: "idle", active_agent: null, last_event: null, last_event_at: null, session_id: null } });
  assert.equal(agentLabel(p), "idle");
});

test("bucketTitle covers all four buckets", () => {
  assert.equal(bucketTitle("active"), "Active");
  assert.equal(bucketTitle("in_flight"), "In Flight");
  assert.equal(bucketTitle("stale"), "Stale");
  assert.equal(bucketTitle("cold"), "Cold Storage");
});

function radar(updatedAt: string): Radar {
  return { schema_version: 1, updated_at: updatedAt, scan_duration_ms: 0, projects: [] };
}

test("isStale is false just under the threshold", () => {
  const now = new Date("2026-08-07T12:00:00Z");
  const updated = new Date(now.getTime() - 23 * 3_600_000).toISOString();
  assert.equal(isStale(radar(updated), now), false);
});

test("isStale is true at/just over the threshold", () => {
  const now = new Date("2026-08-07T12:00:00Z");
  const updated = new Date(now.getTime() - (24 * 3_600_000 + 1000)).toISOString();
  assert.equal(isStale(radar(updated), now), true);
});

test("isStale honors a custom threshold", () => {
  const now = new Date("2026-08-07T12:00:00Z");
  const updated = new Date(now.getTime() - 2 * 3_600_000).toISOString();
  assert.equal(isStale(radar(updated), now, 1), true);
  assert.equal(isStale(radar(updated), now, 5), false);
});
