//! VS Code Copilot sensor. Mirrors `src/petridish/sensors/copilot.py`.
//! Per `IMPLEMENTATION_PLAN.md` §0 finding F6: `workspaceStorage/<hash>/` dirs containing a
//! `chatSessions/` subdir are attributable via the sibling `workspace.json`'s
//! `{"folder": "file://..."}` URI — percent-decoded (e.g. `%20` -> space), not string-sliced.

use crate::config::Config;
use crate::schema::AgentSignal;
use std::collections::HashMap;
use std::path::Path;

/// Walks `workspace_storage_dir` (`~/Library/Application Support/Code/User/workspaceStorage/`
/// on macOS). Per hash directory: skip if no `chatSessions/` subdir, skip if no
/// `workspace.json`, skip multi-root workspaces (v1 limitation — `workspace.json` has more
/// than one folder), skip on malformed JSON (never raise). Otherwise percent-decode the
/// `folder` `file://` URI into a real filesystem path (use the `url` crate — do not
/// string-slice `%XX` sequences), resolve via `discovery::resolve_root`, and set the
/// signal's `at` to the newest mtime under `chatSessions/`. Two hashes resolving to the same
/// root fold to one signal, newest wins. Missing `workspace_storage_dir` => empty map.
pub fn scan(_workspace_storage_dir: &Path, _config: &Config) -> HashMap<String, AgentSignal> {
    todo!("R7: per-hash chatSessions+workspace.json check, url-crate percent-decode, resolve_root fold, skip multi-root")
}
