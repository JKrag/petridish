import { test } from "node:test";
import assert from "node:assert/strict";
import type { Project } from "../src/types.ts";
import { resolveProjectPath } from "../src/lib/resolvePath.ts";

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

test("exact name match wins, even when substring match also exists", () => {
  const projects: Project[] = [
    project({ id: "a", name: "petri", path: "/repos/petri", last_activity_at: "2026-01-01T00:00:00Z" }),
    project({ id: "b", name: "petridish", path: "/repos/petridish", last_activity_at: "2026-01-01T00:00:00Z" }),
    project({ id: "c", name: "other", path: "/repos/petri-path", last_activity_at: "2026-01-01T00:00:00Z" }),
  ];
  // "petri" is an exact name match on project (a), even though "petridish"
  // contains "petri" as a substring. The path /repos/petri-path also contains
  // "petri", but exact name wins outright.
  assert.equal(resolveProjectPath(projects, "petri"), "/repos/petri");
});

test("case-insensitive substring match on name", () => {
  const projects: Project[] = [
    project({ id: "a", name: "PetriDish", path: "/repos/petridish" }),
    project({ id: "b", name: "PetriNet", path: "/repos/petrinet" }),
  ];
  assert.equal(resolveProjectPath(projects, "petri"), "/repos/petridish");
  assert.equal(resolveProjectPath(projects, "PETRIDISH"), "/repos/petridish");
});

test("case-insensitive substring match on path when no name matches", () => {
  const projects: Project[] = [
    project({ id: "a", name: "frontend", path: "/home/user/repos/petri-project" }),
    project({ id: "b", name: "backend", path: "/home/user/repos/api-service" }),
  ];
  assert.equal(resolveProjectPath(projects, "petri"), "/home/user/repos/petri-project");
});

test("no match returns null", () => {
  const projects: Project[] = [
    project({ id: "a", name: "alpha", path: "/a" }),
    project({ id: "b", name: "beta", path: "/b" }),
  ];
  assert.equal(resolveProjectPath(projects, "zzzzz"), null);
});

test("tie-breaker: most recent last_activity_at wins at the same tier", () => {
  // Same exact-match pitfall as above: neither fixture name may equal the
  // query "thing" exactly, or priority-1 short-circuits before the
  // recency tie-break this test exercises ever runs.
  const projects = [
    project({ id: "old", name: "old-thing", path: "/old-thing", last_activity_at: "2025-01-01T00:00:00Z" }),
    project({ id: "new", name: "new-thing", path: "/new-thing", last_activity_at: "2026-12-31T23:59:59Z" }),
  ];
  assert.equal(resolveProjectPath(projects, "thing"), "/new-thing");
});

test("a project with last_activity_at null sorts after one with a real timestamp", () => {
  // Neither name is an exact match for the query, so both land as
  // substring matches at the same tier and the null-vs-real-timestamp
  // tie-break actually gets exercised (a fixture named exactly "thing"
  // would short-circuit on priority-1 exact match before ever reaching
  // the tie-break code this test is meant to cover).
  const projects = [
    project({ id: "a", name: "old-thing", path: "/a", last_activity_at: null }),
    project({ id: "b", name: "new-thing", path: "/b", last_activity_at: "2026-12-31T23:59:59Z" }),
  ];
  assert.equal(resolveProjectPath(projects, "thing"), "/b");
});

test("path-match excludes projects already matched by name at a higher tier", () => {
  const projects: Project[] = [
    project({ id: "a", name: "petri", path: "/repos/petridish", last_activity_at: "2026-12-31T00:00:00Z" }),
    project({ id: "b", name: "other", path: "/home/petri-clone", last_activity_at: "2026-01-01T00:00:00Z" }),
  ];
  // "petri" exactly matches project a by name (only one exact hit → wins).
  assert.equal(resolveProjectPath(projects, "petri"), "/repos/petridish");

  // Same query — exact match wins regardless of path-clone being "newer" or not.
  projects[0].last_activity_at = "2025-01-01T00:00:00Z";
  assert.equal(resolveProjectPath(projects, "petri"), "/repos/petridish");
});

test("empty query matches every project (empty string is a substring of every string)", () => {
  // Same semantics as cli.py's _cmd_path: "" in p.name.lower() is True for
  // any name, so an empty query is not "no match" — matching JS's
  // String.includes("") behavior. In practice this command's Raycast
  // argument is `required: true`, so an empty submission shouldn't reach
  // here, but the pure function itself must stay consistent regardless.
  const projects: Project[] = [
    project({ id: "a", name: "alpha", path: "/a" }),
  ];
  assert.equal(resolveProjectPath(projects, ""), "/a");
});
