/**
 * Mirrors petridish's ~/.petridish/projects.json schema (src/petridish/schema.py).
 *
 * Hand-maintained, not generated — there is no shared schema tooling between
 * the Python daemon and this extension. Field names and nesting must be kept
 * in sync with Radar.to_dict() by hand if schema.py changes.
 */

export const STATUS_BUCKETS = ["active", "in_flight", "stale", "cold"] as const;
export type StatusBucket = (typeof STATUS_BUCKETS)[number];

export interface GitState {
  is_repo: boolean;
  branch: string | null;
  is_dirty: boolean;
  uncommitted_files: number;
  last_commit_at: string | null;
  mine_last_commit_at: string | null;
  github_url: string | null;
}

export interface AgentState {
  state: "working" | "recent" | "idle";
  active_agent: string | null;
  last_event: string | null;
  last_event_at: string | null;
  session_id: string | null;
}

export interface Project {
  id: string;
  name: string;
  path: string;
  category: string;
  is_foreign: boolean;
  git: GitState;
  agent: AgentState;
  last_activity_at: string | null;
  status_bucket: StatusBucket;
}

/**
 * Account-wide Claude usage, sourced from Claude Code's own
 * `~/.claude/last-status.json`.
 *
 * Every field is nullable and the whole block may be null: the daemon reads an
 * undocumented internal file of another program, so any field can vanish on a
 * Claude Code upgrade. Treat null as "unknown", never as zero.
 *
 * These figures are account-global — render them in a header, never on a
 * project row.
 */
export interface QuotaState {
  /** When Claude Code wrote the numbers, not when petridish read them. The
   *  file only updates while a session is running, so this can be hours old
   *  while the rest of the radar is a minute old. */
  measured_at: string | null;
  five_hour_used_pct: number | null;
  five_hour_resets_at: string | null;
  seven_day_used_pct: number | null;
  seven_day_resets_at: string | null;
  context_used_pct: number | null;
}

export interface Radar {
  schema_version: number;
  updated_at: string;
  scan_duration_ms: number;
  projects: Project[];
  /** Absent in files written before this field existed, and null whenever the
   *  sensor found nothing — so read it defensively. */
  quota?: QuotaState | null;
}
