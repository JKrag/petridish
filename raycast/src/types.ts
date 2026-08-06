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

export interface Radar {
  schema_version: number;
  updated_at: string;
  scan_duration_ms: number;
  projects: Project[];
}
