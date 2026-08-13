//! `swab-hook-rs` — the fast hook path. Mirrors `src/petridish/hook.py`.
//!
//! Single-writer invariant (`CLAUDE.md` #1): this binary must NEVER open `projects.json` for
//! writing — only `swab-rs scan` may do that. This binary's only job is one `O_APPEND` write
//! of one JSON line to `events.ndjson`.
//!
//! No CLI flags — reads the hook-event JSON body from stdin (`cwd`, `session_id`/`sessionId`,
//! `hook_event_name`). Must ALWAYS exit 0, even on malformed/empty stdin or an append failure
//! — this hook is chained after several other Claude Code hook consumers and must never break
//! that chain (mirrors hook.py's bare `except BaseException: return 0`).

fn main() {
    todo!("R5/R9: read stdin JSON, build RawHookEvent (drop event if cwd missing), append_event, always exit 0")
}
