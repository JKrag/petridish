import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { readRadar, StateFileMissingError } from "../src/lib/readProjects.ts";

test("readRadar throws StateFileMissingError with the canonical message when absent", () => {
  const missing = path.join(os.tmpdir(), "petri-test-does-not-exist", "projects.json");
  assert.throws(
    () => readRadar(missing),
    (err: unknown) => err instanceof StateFileMissingError && (err as Error).message === `no state file at ${missing}; run 'swab scan' first`,
  );
});

test("readRadar parses a fixture file", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "petri-test-"));
  const statePath = path.join(dir, "projects.json");
  fs.writeFileSync(
    statePath,
    JSON.stringify({ schema_version: 1, updated_at: "2026-08-07T00:00:00Z", scan_duration_ms: 5, projects: [] }),
  );
  const radar = readRadar(statePath);
  assert.equal(radar.schema_version, 1);
  assert.deepEqual(radar.projects, []);
});
