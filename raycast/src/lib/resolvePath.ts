/**
 * Resolve a query to a single project path.
 *
 * Port of `petridish.cli._cmd_path` priority rules:
 *
 *   1. Exact case-sensitive match on `name` (only when exactly one matches;
 *      a single exact winner is more specific than any fuzzy match).
 *   2. Case-insensitive substring match on `name`.
 *   3. Case-insensitive substring match on `path`.
 *
 * Ties within a tier break by most recent `last_activity_at`. Projects with
 * a `null` timestamp sort after all projects with a real timestamp.
 *
 * A project that already appeared in the name-match bucket is not re-added
 * to the path-match bucket — same rule as `_cmd_path` (`if p not in candidates`).
 */

import type { Project } from "../types.ts";

export function resolveProjectPath(
  projects: Project[],
  query: string,
): string | null {
  // Priority 1: exact case-sensitive match on name.
  const exact = projects.filter((p) => p.name === query);
  if (exact.length === 1) return exact[0].path;
  // zero or multiple: fall through to fuzzy tiers.

  const needle = query.toLowerCase();

  // Priority 2: case-insensitive substring on name.
  const nameMatches = projects.filter((p) =>
    p.name.toLowerCase().includes(needle),
  );

  // Priority 3: case-insensitive substring on path.
  const pathMatches = projects.filter((p) =>
    p.path.toLowerCase().includes(needle),
  );

  // Deduplicate: a project already placed at rank 0 never gets rank 1.
  const seen = new Map<string, boolean>();
  const candidates: Array<[0 | 1, Project]> = [];
  for (const p of nameMatches) {
    if (!seen.has(p.id)) {
      seen.set(p.id, true);
      candidates.push([0, p]);
    }
  }
  for (const p of pathMatches) {
    if (!seen.has(p.id)) {
      seen.set(p.id, true);
      candidates.push([1, p]);
    }
  }

  if (candidates.length === 0) return null;

  // rank ascending, last_activity_at descending (null sorts after everything).
  candidates.sort((a, b) => {
    const rankDelta = a[0] - b[0];
    if (rankDelta !== 0) return rankDelta;
    const atA = activityAtMs(a[1]);
    const atB = activityAtMs(b[1]);
    if (atA === atB) return 0;
    if (atA === -1) return 1; // null sorts last
    if (atB === -1) return -1;
    return atB > atA ? 1 : -1; // most recent first
  });

  return candidates[0]![1].path;
}

/**
 * Parse `last_activity_at` to epoch ms. A `null` timestamp (or any parse
 * failure) returns -1, which sorts below every real ms value.
 */
function activityAtMs(p: Project): number {
  if (!p.last_activity_at) return -1;
  const ms = Date.parse(p.last_activity_at);
  return Number.isNaN(ms) ? -1 : ms;
}
