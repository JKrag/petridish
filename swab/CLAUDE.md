# swab

The scanner: the only thing that writes `projects.json` (single-writer invariant below).
See the root `CLAUDE.md` for the workspace-wide picture; this file is the `swab/`-specific
detail that only needs to load when you're working in this crate.

## Non-negotiable invariants

These encode findings verified on the real machine. Violating one produces code that passes
tests and is still wrong. Invariants 1-5 apply here; invariant 6 has been superseded (see
note).

1. **Single writer.** Only `swab scan` writes `projects.json`, via temp-file + atomic
   rename. `swab-hook` appends one line to `events.ndjson` and nothing else. Never make the
   hook touch `projects.json` — three other hook consumers already share these events.
2. **Never parse a path out of a `~/.claude/projects/` dirname.** The slug encodes `/` and
   `-` identically and is not reversible. Read `cwd` from the JSONL contents.
3. **`cwd` varies within one transcript.** Take it from the *last* parseable line, then run
   it through `resolve_root()` so monorepo subdirs collapse to one project.
4. **Truncated trailing JSONL lines are normal**, not errors — live sessions are being
   appended to as you read. Skip and fall back to the previous line.
5. **Sensors degrade, never abort.** A failing sensor yields `null` fields; the tick still
   writes a complete file.
6. ~~**`git` calls use `subprocess.run` with `check=False` and a 5s timeout.**~~ Superseded:
   `swab/src/git.rs` now calls `gix` in-process (no subprocess, no timeout construct) for
   everything except nothing — there's no CLI fallback left at all. The invariant this
   protected still holds in spirit: a git failure degrades to `GitState { is_repo: false,
   .. }`, never a panic or an exception, enforced by `gix::open`'s `Result` and `?`-free
   fallback matching throughout `git.rs`.
