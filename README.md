# petridish

A local monitoring daemon for macOS. It crawls your project roots every minute, tracks git
state, senses which AI coding agents are actually working, and aggregates it all into
`~/.petridish/projects.json` — which a terminal dashboard, a menu-bar plugin and a Raycast
extension then read.

Built for the situation where you have dozens of small experiments scattered across the
filesystem and no idea which ones are alive, which have uncommitted work, and which agent
is currently waiting on you.

macOS only, by nature: launchd and `~/Library` are load-bearing.

## Install

```sh
brew install jkrag/tap/petridish
petridish install
```

`petridish install` is the step that wires the tool into the machine. It:

- creates `~/.petridish/` with a commented-out default `config.toml`
- registers a launchd job that runs `swab scan` every 60 seconds, logging to
  `~/.petridish/daemon.log`
- adds Claude Code hook entries to `~/.claude/settings.json`, tagged with the literal
  marker `# petridish`, **without disturbing any other hook consumer** already configured
  there
- installs the xbar/SwiftBar menu-bar plugin (skip it with `--no-menubar-plugin`)

It backs up `~/.claude/settings.json` once, to `~/.petridish/settings.json.backup`, before
touching it. That backup is a safety artifact for you — uninstall never reads it back
automatically. See [Uninstall semantics](#uninstall-semantics).

Re-running `petridish install` is safe and is the right move after any upgrade that
relocates the binaries.

<details>
<summary>Installing from a checkout instead</summary>

```sh
cargo install --path petridish-cli --locked   # petridish
cargo install --path swab --locked            # swab, swab-hook
cargo install --path petri --locked           # petri
petridish install
```

**`--locked` is required, not optional.** `cargo install` ignores `Cargo.lock` by default
and re-resolves from scratch; a transitive `gix` dependency (`bisync`) has had its matching
versions yanked from crates.io, so the default resolution fails outright. The lockfile pins
a working set.
</details>

## Commands

Four binaries, each with one job:

| Binary | Role |
| --- | --- |
| `petridish` | Install, uninstall, health-check, and render the menu bar |
| `swab` | The scanner. The **only** thing that writes `projects.json` |
| `swab-hook` | The Claude Code hook. Appends one line to `events.ndjson`, nothing else |
| `petri` | The terminal dashboard |

```sh
petridish install       # wire up launchd + the Claude Code hook + the menu bar
petridish uninstall     # remove all of that, leaving ~/.petridish intact
petridish doctor        # is the install intact?
petridish menubar       # print xbar plugin text for the current state

swab scan               # run one tick, write ~/.petridish/projects.json
swab list [--bucket B] [--all] [--json]
swab path <query>       # print the best-matching project's path
swab doctor             # health-check config, roots, state freshness, hook wiring
swab config             # print the config file location and an example

petri                   # the dashboard
```

`swab list` sample output:

```
bucket     name             agent                  branch  dirty
---------  ---------------  ---------------------  ------  -----
active     petridish        claude-code (working)  master  *
in_flight  fastfood-filter  copilot (idle)         main
cold       old-experiment   idle                   main
```

Two `doctor` commands, deliberately: `swab doctor` answers "is the scanner healthy"
(config parses, roots exist, state file is fresh), `petridish doctor` answers "is the
install intact" (binaries resolve, the plist points somewhere real, every hook event is
registered).

### Uninstall semantics

`petridish uninstall` unloads the launchd job, deletes its plist, and **structurally
removes only the hook entries carrying the `# petridish` marker** from `settings.json`. It
does not restore the backup verbatim. That distinction matters: if you or another tool
edited `settings.json` after installing, a verbatim restore would silently discard that
edit.

`~/.petridish/` — config, state, the backup — is never deleted, so a later reinstall picks
up where you left off.

## Frontends

- **`petri`** — the terminal dashboard. Two screens, filtering, collapsible sections,
  worktree nesting, an activity feed. `petri/SPEC.md` is authoritative for its behaviour.
- **Menu bar** — xbar/SwiftBar. See [`integrations/xbar/`](integrations/xbar/).
- **Raycast** — a list view and a jump-to-project command. See
  [`integrations/raycast/`](integrations/raycast/).

All three are read-only. `swab scan` is the single writer, always.

## Shell integration: quick-jump between projects

A `cd`-in-your-current-shell project switcher, for opening a new terminal and jumping
straight to a project without remembering its path — including when the folder name
doesn't match what you'd search for. Not installed by anything above; add it to `~/.zshrc`
yourself (needs `fzf` and `jq`: `brew install fzf jq`):

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

- `pj` alone opens an fzf picker over every project `swab` knows about — folder name, and
  (when the project has a GitHub remote) its `org/repo` alongside it. Matching is
  restricted to the name column, so a query never accidentally matches something buried in
  the filesystem path.
- `pj <query>` prefilters; if exactly one project matches it jumps straight there. Because
  the GitHub org is searchable, `pj eficode-academy/` narrows to every repo under that org
  even if they are scattered across folders.
- The query is auto-prefixed with `'` (fzf's exact-match token), so `pj petri` matches the
  literal substring rather than fzf's scattered-letter fuzzy matching, which matched too
  much to auto-jump reliably. Backspace it in the picker if you want loose matching.

## Config

`~/.petridish/config.toml` — entirely optional; every field has a default. Run `swab
config` for the full field reference, sourced from `swab/src/config.rs`'s own
`Config::default()` so it cannot drift out of sync with the code.

## Development

A cargo workspace; one toolchain, no other language runtime required.

```sh
make check     # fmt-check + clippy -D warnings + the full test suite
make fmt       # reformat
```

`make check` is exactly what CI runs, so green locally means green in CI.

This repo uses `.git-blame-ignore-revs` to keep `git blame` readable across the bulk
formatting commit. Configure it once:

```sh
git config blame.ignoreRevsFile .git-blame-ignore-revs
```

## Docs

- `ARCHITECTURE.md` — architecture, empirical findings, the `projects.json` schema, and
  the distribution/installer requirements (§8). The authoritative reference.
- `petri/SPEC.md` — the dashboard's spec, authoritative for its screens and behaviour.
- `CONTRIBUTING.md` — what to run, and what must not break.
- `CLAUDE.md` — non-negotiable invariants for anyone changing this code.
- `ADR-0001`…`ADR-0004` — the decisions that are expensive to revisit.

## License

GPL-3.0-or-later — see `LICENSE`.
