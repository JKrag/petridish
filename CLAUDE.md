# project-radar

Local monitoring daemon for macOS: crawls project roots, tracks git state, senses AI agent
activity, and aggregates into `~/.project-radar/projects.json`.

**Read `IMPLEMENTATION_PLAN.md` before writing any code.** It is the authoritative spec and
supersedes `DESIGN.md` wherever they disagree. Build only the module you were assigned.

## Stack & layout

- **Python 3.12+, stdlib only** for all runtime code. `pytest` is the sole dev dependency.
  Do not add runtime dependencies — no `watchdog`, no `pydantic`, no `click`, no `rich`.
  Zero deps is a deliberate constraint so each module is verifiable with no env setup.
- Source in `src/radar/`, sensors in `src/radar/sensors/`, tests in `tests/`.
- Console scripts: `radar` (CLI) and `radar-hook` (fast hook path).

## Non-negotiable invariants

These encode findings verified on the real machine (see `IMPLEMENTATION_PLAN.md` §0).
Violating one produces code that passes tests and is still wrong.

1. **Single writer.** Only the daemon writes `projects.json`, via temp-file + `os.replace()`.
   `radar-hook` appends one line to `events.ndjson` and nothing else. Never make the hook
   touch `projects.json` — three other hook consumers already share these events.
2. **Never parse a path out of a `~/.claude/projects/` dirname.** The slug encodes `/` and
   `-` identically and is not reversible. Read `cwd` from the JSONL contents.
3. **`cwd` varies within one transcript.** Take it from the *last* parseable line, then run
   it through `resolve_root()` so monorepo subdirs collapse to one project.
4. **Truncated trailing JSONL lines are normal**, not errors — live sessions are being
   appended to as you read. Skip and fall back to the previous line.
5. **Sensors degrade, never abort.** A failing sensor yields `null` fields; the tick still
   writes a complete file.
6. **`git` calls** use `subprocess.run` with `check=False` and a 5s timeout. A git failure
   is a `GitState(is_repo=False)`, never an exception.

## Testing

Real fixtures, not mocks: `git init` actual repos in tmpdirs with pinned author/date env
vars; write actual fixture transcript files. Mocked subprocess output would have hidden
every finding in §0 of the plan.

Each module is done when its `pytest tests/test_<module>.py -q` exits 0.

## Engineering integrity

Correctness over green checks. Do not weaken, skip, or delete a test to make it pass — if a
check fails, fix the code. If a module cannot be built as specified, **stop and escalate
with the reason**; do not narrow the spec, stub a sensor, or silently substitute a simpler
approach. Any deliberate shortcut needs a comment at the site and a note in the summary.
