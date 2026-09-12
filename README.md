# petridish

A local monitoring daemon. It crawls your project roots every minute, tracks git state,
senses which AI coding agents are actually working, and aggregates it all into
`~/.petridish/projects.json` — which a terminal dashboard, a menu-bar plugin and a Raycast
extension then read.

Built for the situation where you have dozens of small experiments scattered across the
filesystem and no idea which ones are alive, which have uncommitted work, and which agent
is currently waiting on you.

**`swab`** (the scanner) and **`petri`** (the terminal dashboard) are cross-platform — no
native-macOS dependency. **`petridish`** wires the other two into launchd, the menu bar,
and the Claude Code hook: `install`/`uninstall` only support macOS today (they use launchd
and `~/Library`, and exit with an error on other platforms) — there's no Linux equivalent
yet, see [#75](https://github.com/JKrag/petridish/issues/75). `doctor` and `menubar` already
run cross-platform: `doctor` reports its launchd/plist/menu-bar checks as "not applicable"
where they don't apply instead of failing, and `menubar` prints an explicit message instead
of plugin text nothing will read.

| | macOS | Linux |
| --- | --- | --- |
| `swab` (scanner), `petri` (dashboard) | ✅ | ✅ |
| `petridish install` / `uninstall` | ✅ | ❌ — no Linux support yet ([#75](https://github.com/JKrag/petridish/issues/75)); do it by hand, see [Linux](#linux) |
| `petridish doctor` | ✅ full checks | ✅ — macOS-only checks report "not applicable" |
| `petridish menubar` | ✅ | ❌ — prints a "macOS-only" message and exits 0 |

**On Linux, start at the [Linux](#linux) section below** — it covers the manual setup (a
systemd timer or cron, plus a one-time hook registration) that replaces what `petridish
install` does on macOS. The rest of this README up to that point is macOS-flavored.

## Install (macOS)

> On Linux, `petridish install` doesn't apply — skip to [Linux](#linux) instead.

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
automatically. See [Uninstall semantics (macOS)](#uninstall-semantics-macos).

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

**Use `--locked`.** `cargo install` ignores `Cargo.lock` by default and re-resolves from
scratch, so without it you get whatever is newest rather than the versions CI tested.

It used to be strictly mandatory: a transitive `gix` dependency (`bisync`) had its matching
versions yanked from crates.io, and an unlocked resolve failed outright. `gix` 0.87 dropped
that dependency, so the hard failure is gone — but reproducibility is still the reason to
pass it.
</details>

## Commands

Four binaries, each with one job:

| Binary | Role | Platform |
| --- | --- | --- |
| `petridish` | Install, uninstall, health-check, and render the menu bar | macOS (`doctor` also runs on Linux, degraded — see [Linux](#linux)) |
| `swab` | The scanner. The **only** thing that writes `projects.json` | Cross-platform |
| `swab-hook` | The Claude Code hook. Appends one line to `events.ndjson`, nothing else | Cross-platform |
| `petri` | The terminal dashboard | Cross-platform |

```sh
petridish install       # macOS only — wire up launchd + the Claude Code hook + the menu bar
petridish uninstall     # macOS only — remove all of that, leaving ~/.petridish intact
petridish doctor        # is the install intact? (macOS/plist checks skip on Linux)
petridish menubar       # macOS only — print xbar plugin text for the current state

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

### Uninstall semantics (macOS)

`petridish uninstall` unloads the launchd job, deletes its plist, and **structurally
removes only the hook entries carrying the `# petridish` marker** from `settings.json`. It
does not restore the backup verbatim. That distinction matters: if you or another tool
edited `settings.json` after installing, a verbatim restore would silently discard that
edit.

`~/.petridish/` — config, state, the backup — is never deleted, so a later reinstall picks
up where you left off.

## Linux

`swab` (the scanner) and `petri` (the dashboard) work fully on Linux. There's no installer
yet, so the four steps below — build, schedule the scan, register the Claude Code hook, run
`petri` — are what `petridish install` would otherwise do for you. Each is a one-time setup.

**1. Install the binaries.**

```sh
cargo install --path swab --locked            # swab, swab-hook
cargo install --path petri --locked           # petri
cargo install --path petridish-cli --locked   # petridish (for `doctor`, step 4)
```

**2. Schedule `swab scan` to run every 60 seconds.** A systemd user timer is the
recommended way:

`~/.config/systemd/user/petridish-scan.service`:

```ini
[Unit]
Description=petridish scan

[Service]
Type=oneshot
ExecStart=%h/.cargo/bin/swab scan
```

`~/.config/systemd/user/petridish-scan.timer`:

```ini
[Unit]
Description=Run petridish scan every 60 seconds

[Timer]
OnBootSec=10
OnUnitActiveSec=60
AccuracySec=1

[Install]
WantedBy=timers.target
```

```sh
systemctl --user daemon-reload
systemctl --user enable --now petridish-scan.timer
journalctl --user -u petridish-scan -f   # tail the logs
```

By default this only scans while you're logged in. To also have it run before login (e.g.
after a reboot with no interactive session), enable lingering:

```sh
loginctl enable-linger "$USER"
```

No systemd? A cron entry works too — wrap it in `flock` so two ticks can never overlap and
corrupt each other's state:

```
* * * * * flock -n /tmp/petridish-scan.lock $HOME/.cargo/bin/swab scan >> $HOME/.petridish/daemon.log 2>&1
```

**3. Register the Claude Code hook.** This is what lets petridish sense when an agent is
running or waiting on you — without it, git-based facts (branch, dirty state) still work,
but agent activity won't show up. Add the following to `~/.claude/settings.json`, merging it
into whatever `hooks` object is already there (don't overwrite the file). Replace
`/home/you/.cargo/bin/swab-hook` with the real path from `which swab-hook`:

```json
{
  "hooks": {
    "PreToolUse": [
      { "hooks": [ { "type": "command", "command": "'/home/you/.cargo/bin/swab-hook' # petridish" } ] }
    ],
    "Stop": [
      { "hooks": [ { "type": "command", "command": "'/home/you/.cargo/bin/swab-hook' # petridish" } ] }
    ],
    "Notification": [
      { "hooks": [ { "type": "command", "command": "'/home/you/.cargo/bin/swab-hook' # petridish" } ] }
    ],
    "PermissionRequest": [
      { "hooks": [ { "type": "command", "command": "'/home/you/.cargo/bin/swab-hook' # petridish" } ] }
    ]
  }
}
```

Keep the path quoted exactly like that (single quotes, no `~`) and the four event names
spelled exactly as shown — `swab doctor` checks for them by name to confirm the hook is
wired up correctly.

**4. Run it.**

```sh
petri             # the dashboard
swab doctor       # sanity-check config, roots, and state freshness
petridish doctor  # sanity-check the hook + binaries (macOS-only checks report "not applicable")
```

That's it — `petri` and `swab` behave identically to macOS from here on.

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

`make check` is the fast loop. `make check-all` adds the gates that need extra
tooling (cargo-deny, an MSRV toolchain, node) and is what CI runs in full — run it
before opening a PR.

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
