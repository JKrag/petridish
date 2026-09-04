# petridish

Local monitoring daemon for macOS: crawls project roots, tracks git state, senses AI agent
activity, and aggregates into `~/.petridish/projects.json`.

## Install (dev)

Two separate pieces, two separate toolchains:

**`swab` / `swab-hook`** (the scanner) are Rust, built from `swab/`:

```sh
cargo install --path swab --locked
```

This puts `swab` and `swab-hook` on `~/.cargo/bin` (verify it's on `PATH`). **`--locked`
is required, not optional:** `cargo install` ignores `Cargo.lock` by default and
re-resolves from scratch, and a transitive `gix` dependency (`bisync`) has had its
matching versions yanked from crates.io, so the default resolution fails outright. The
lockfile pins a working set. Same flag for `petri` below, for the same reason.

```sh
swab --help
```

**`petri`** (the TUI dashboard) is Rust/ratatui and is built — both screens, filtering,
collapsible sections, worktree nesting and the activity feed. `petri/SPEC.md` is
authoritative for its behaviour:

```sh
cargo install --path petri --locked
```

This puts `petri` on `~/.cargo/bin` alongside `swab`. It only ever reads
`~/.petridish/projects.json`; `swab scan` remains its sole writer.

**`petripy`** is the Python original, renamed to free up the `petri` name. It is
deprecated and frozen — kept installed as a fallback while the Rust build earns trust,
not developed further (see `CONTEXT.md`'s `petripy` entry for its deletion trigger). The
same `uv` install also provides `petridish-installer`, which is used below, so this step
is still needed even if you never run `petripy`:

```sh
uv tool install --editable .
```

This puts `petripy` and `petridish-installer` on `~/.local/bin` (already first on `PATH`
for most `uv` setups).

## Wire up the launchd job + Claude Code hook

```sh
./install.sh              # install: launchd job (60s tick) + Claude Code PreToolUse/Stop hook
./install.sh --uninstall  # remove both, cleanly
```

`install.sh` requires `swab`/`swab-hook` and `petridish-installer` to already be on `PATH`
(see above). It:

- creates `~/.petridish/` and writes a default `config.toml` if one isn't already there
- backs up `~/.claude/settings.json` once, to `~/.petridish/settings.json.backup`, before ever
  touching it — this is a safety artifact, not something `--uninstall` reads back automatically
  (see "Uninstall semantics" below)
- appends hook entries tagged with the literal marker `# petridish`, without disturbing any
  other hook consumers already configured (this machine has three: pixtuoid, statusbar,
  notchbar)
- registers `resources/com.petridish.daemon.plist` with launchd (`StartInterval` 60s,
  `RunAtLoad`, logs to `~/.petridish/daemon.log`)

Both the install and the launchd/hook step are idempotent — running `install.sh` twice is safe
(the second run detects the hook marker and the launchd label already loaded, and changes
nothing). One caveat: the plist file itself is always rewritten with the current `swab` path, but
`launchd` won't pick up that change on an already-loaded label — if `swab`'s absolute path
changes (e.g. after `cargo install --path swab` relocates it), run
`./install.sh --uninstall && ./install.sh` rather than just `./install.sh` again.

### Uninstall semantics

`--uninstall` unloads the launchd job, deletes its plist, and **structurally removes only the
hook entries carrying the `# petridish` marker** from `settings.json` — it does not restore the
pre-install backup verbatim. That distinction matters: if you (or another tool) edited
`settings.json` after installing petridish, a verbatim restore would silently discard that edit.
Structural removal only ever touches what petridish itself added.

`~/.petridish/` (config, state, the settings.json backup) is never deleted by `--uninstall` — it
survives so a later reinstall picks up where you left off.

## CLI

```sh
swab scan              # run a tick, write ~/.petridish/projects.json
swab list [--bucket B] [--all] [--json]
swab path <query>      # print the best-matching project's path
swab doctor            # health-check config, roots, state freshness, hook wiring
swab config            # print the config file location, its fields, and an example
```

`swab list` sample output:

```
bucket     name             agent                  branch  dirty
---------  ---------------  ---------------------  ------  -----
active     petridish        claude-code (working)  master  *
in_flight  fastfood-filter  copilot (idle)         main
cold       old-experiment   idle                   main
```

## Shell integration: quick-jump between projects

A `cd`-in-your-current-shell project switcher, for opening a new terminal and jumping
straight to a project without remembering its path — including when the folder name
doesn't match what you'd search for (e.g. this repo's folder is still `project-radar`,
but its GitHub remote is `JKrag/petridish`, and `pj` matches on both). Not installed by
anything above — add it yourself to `~/.zshrc` (requires `fzf` and `jq`, e.g.
`brew install fzf jq`):

```sh
pj() {
  local sel
  sel=$(swab list --all --json \
    | jq -r '.[] | (.git.github_url // "") as $gh
        | ($gh | if . == "" then "" else (split("/") | .[-2:] | join("/")) end) as $org_repo
        | (if $org_repo == "" then .name else "\(.name)  (\($org_repo))" end) as $label
        | "\($label)\t\(.path)"' \
    | awk -F'\t' '{printf "%-45s\t%s\n", $1, $2}' \
    | fzf --prompt="jump to project> " --delimiter=$'\t' --nth=1 \
          --query="'$1" --select-1 --exit-0 \
    | cut -f2)
  [[ -z "$sel" ]] && return 1
  cd "$sel" || return 1
}
```

- `pj` alone opens an fzf picker over every project `swab` knows about — folder name,
  and (when the project has a GitHub remote) its `org/repo` alongside it, e.g.
  `project-radar  (JKrag/petridish)`. The trailing column is the path that gets `cd`'d
  into; matching is restricted to the name/org-repo column so a query never accidentally
  matches something buried in the filesystem path.
- `pj <query>` prefilters to that query; if exactly one project matches, it jumps there
  directly with no picker. Because the GitHub org is searchable, `pj eficode-academy/`
  narrows to every repo under that org even if they're scattered across folders instead
  of tidily grouped by org.
- The query is auto-prefixed with a leading `'` (fzf's exact-match token syntax), so
  `pj petri` only matches names/org-repos containing the literal substring `petri` —
  not fzf's usual scattered-letters fuzzy matching, which was matching too many
  unrelated projects to reliably auto-jump. If you want loose fuzzy matching to browse
  around once the picker is open, just backspace that leading `'` yourself.
- Relies on `swab list --json`'s `name`/`git.github_url`/`path` fields, so it stays in
  sync with whatever `swab scan` last wrote to `~/.petridish/projects.json` — no
  separate index to maintain.

## Config

`~/.petridish/config.toml` — entirely optional; every field has a default. Run
`swab config` for the full field reference (sourced from `swab/src/config.rs`'s own
`Config::default()`, so it can't drift out of sync with the code) and an example.
`install.sh` writes a commented-out template there on first install (see
`DEFAULT_CONFIG_TOML` in `src/petridish/installer.py`).

## Docs

- `ARCHITECTURE.md` — language-agnostic architecture, empirical findings, the
  `projects.json` schema, and the distribution/installer requirements (§8); the current
  authoritative reference
- `petri/SPEC.md` — the Rust TUI's spec, authoritative for its screens/behavior
- `docs/archive/DESIGN.md` — original pre-implementation pitch doc (Python `Textual`, a
  FastAPI web UI, neither built); kept as historical context, superseded by `ARCHITECTURE.md`
- `docs/archive/IMPLEMENTATION_PLAN.md` — the original all-Python build spec, archived once
  the scanner was ported to Rust
- `CLAUDE.md` — non-negotiable invariants for anyone changing this code

## License

GPL-3.0-or-later — see `LICENSE`.
