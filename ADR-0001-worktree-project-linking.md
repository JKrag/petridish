# Worktree projects: path-convention detection, `parent_path` field, display-only rollup

**Status:** accepted

A worktree under `.worktrees/<name>` is discovered by `swab` as its own independent
`Project` (correctly — `resolve_root` must not collapse it into its parent, since it has
independent git/agent state), but showed up in `petri`/menubar with only its
auto-generated branch-slug name, no indication of which real project it belongs to.

**Detection: path convention, not git-native.** A `Project` is a worktree if its resolved
path contains a `.worktrees/<name>` segment. We considered asking `gix` directly (it
already opens the repo in `git.rs::scan`, and can distinguish a linked worktree's `Kind`
from a main checkout), which would also catch worktrees created outside the convention.
Rejected: `.worktrees/` is already the sanctioned pattern everywhere else in this
ecosystem (crawl-skip in `config.rs`, and the `worktree-provision`/`feature-branch`/
`using-git-worktrees` skills all create worktrees there exclusively). A worktree made by
hand outside that folder is, per the user, deliberate long-term maintenance work with a
meaningful self-chosen name — it should read as a first-class project, not get folded
into worktree-linking heuristics built for the throwaway agentic case.

**Schema: one additive nullable field, `parent_path: string | null`.** Not `parent_id` —
`id` is already documented as unstable across moves, so caching a derived hash of it would
just be a second unstable value; frontends that have `parent_path` can look up the parent
row by `path` directly. Not a new `is_worktree` boolean — non-null `parent_path` already
is that signal. `schema_version` stays at `1`: nothing in either language's reader
actually gates on it, and `#[serde(default)]` on the Rust side keeps a stale
pre-this-change `projects.json` readable by newer binaries.

**Activity rollup ("parent is active if a worktree child is active") lives in `petri`
only, not in `status_bucket`.** A worktree's commits, uncommitted files, and agent
sessions never touch the parent path's own git state or mtimes (separate working
directory, index, and usually branch — only `commondir` is shared), so a parent can sit
`stale`/`cold` indefinitely while active work happens entirely inside a worktree; this is
the normal shape of the feature-branch workflow, not an edge case. We chose to fold "has
an active worktree child" into `petri`'s RUNNING-section membership check rather than
into `swab`'s `status_bucket` computation, because baking it into the schema would change
what `status_bucket` *means* for every consumer (`swab list`, menubar) based on a
not-yet-battle-tested UX call, and `petri`'s current Python/curses build is explicitly
where this project experiments before committing anything to the Rust/schema side.

**Rendering, one shape per surface, decided by feel not consistency:**
- `swab list`: suffix, `name (in parent-name)`.
- `petri`: indented tree, but only inside the RUNNING section, and only when the parent
  is also present there to nest under (worktree active + parent not RUNNING falls back to
  the same suffix style as `swab list`). Other buckets get a count on the parent's own
  row instead of listing children (`catshow-searcher · 3 worktrees`), counting all worktree
  children regardless of which bucket they're individually in.
- menubar: flat prefix, `parent-name / name`, no submenus. Explicitly not doing anything
  fancier here — xbar/SwiftBar has already proven flaky in this project, so this surface
  gets the lowest-risk possible change.
