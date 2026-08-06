/**
 * Pure, framework-free helpers over a Radar/Project — no @raycast/api import
 * anywhere in this file. Mirrors the split used by petridish's petri TUI
 * (src/petridish/tui_state.py): logic that doesn't need a real UI runtime to
 * exercise lives here and is covered by tests/state.test.ts; the Raycast
 * command component (list-projects.tsx) only renders what these return.
 */

import { STATUS_BUCKETS } from "../types.ts";
import type { Project, Radar, StatusBucket } from "../types.ts";

export function groupByBucket(projects: Project[]): Record<StatusBucket, Project[]> {
  const buckets = Object.fromEntries(STATUS_BUCKETS.map((b) => [b, [] as Project[]])) as Record<
    StatusBucket,
    Project[]
  >;
  for (const p of projects) {
    if (p.is_foreign) continue;
    buckets[p.status_bucket].push(p);
  }
  return buckets;
}

export function filterProjects(projects: Project[], query: string): Project[] {
  if (!query) return projects;
  const needle = query.toLowerCase();
  return projects.filter((p) => p.name.toLowerCase().includes(needle));
}

/** Same 24h default threshold as swab doctor / petri (cli.py, tui_state.py). */
export function isStale(radar: Radar, now: Date, thresholdHours = 24): boolean {
  const updatedMs = Date.parse(radar.updated_at);
  const elapsedHours = (now.getTime() - updatedMs) / 3_600_000;
  return elapsedHours >= thresholdHours;
}

/** Same rule as cli.py's _print_table / tui_state.py's format_row. */
export function agentLabel(p: Project): string {
  return p.agent.active_agent ? `${p.agent.active_agent} (${p.agent.state})` : p.agent.state;
}

const BUCKET_TITLES: Record<StatusBucket, string> = {
  active: "Active",
  in_flight: "In Flight",
  stale: "Stale",
  cold: "Cold Storage",
};

export function bucketTitle(bucket: StatusBucket): string {
  return BUCKET_TITLES[bucket];
}
