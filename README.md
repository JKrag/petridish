# petridish

A local monitoring daemon. It crawls your project roots every minute, tracks git state,
senses which AI coding agents are actually working, and aggregates it all into
`~/.petridish/projects.json` — which a terminal dashboard, a menu-bar plugin and a Raycast
extension then read.

Built for the situation where you have dozens of small experiments scattered across the
filesystem and no idea which ones are alive, which have uncommitted work, and which agent
is currently waiting on you.

Three binaries: **`swab`** scans, **`petri`** is the terminal dashboard, and **`petridish`**
installs everything as a background daemon — no need to run anything by hand once it's set
up. Works on both macOS and Linux; see [Install](#install) below.

## Install

**macOS, and Linux on x86_64** — Homebrew tap ([Linuxbrew](https://docs.brew.sh/Homebrew-on-Linux)
on Linux):

```sh
brew install jkrag/tap/petridish
petridish install
```

**Linux on other architectures (e.g. arm64)** — no prebuilt binary yet, only `cargo install`
from a checkout (needs a Rust toolchain): clone this repo, then follow
[Installing from a checkout instead](#installing-from-a-checkout-instead) below, and finish
with `petridish install`.

`petridish install` is the step that wires the tool into the machine. It:

- creates `~/.petridish/` with a commented-out default `config.toml`
- registers a background job that runs `swab scan` every 60 seconds, logging to
  `~/.petridish/daemon.log` — a launchd job on macOS, a `systemd --user` timer
  (`petridish-scan.timer` / `.service`, under `$XDG_CONFIG_HOME/systemd/user/` if that's set,
  else `~/.config/systemd/user/`) on Linux
- adds Claude Code hook entries to `~/.claude/settings.json`, tagged with the literal
  marker `# petridish`, **without disturbing any other hook consumer** already configured
  there
- macOS only: installs the xbar/SwiftBar menu-bar plugin (skip it with
  `--no-menubar-plugin`); Linux has no menu-bar equivalent, so this step is a no-op there

It backs up `~/.claude/settings.json` once, to `~/.petridish/settings.json.backup`, before
touching it. That backup is a safety artifact for you — uninstall never reads it back
automatically. See [Uninstall semantics](#uninstall-semantics).

Re-running `petridish install` is safe and is the right move after any upgrade that
relocates the binaries — on Linux this also picks up a changed scan interval, since it
always `daemon-reload`s and restarts the timer with whatever is on disk.

On Linux, `install` requires a running `systemd --user` session (true on every mainstream
desktop distro — Ubuntu, Fedora, Debian, RHEL and derivatives, Arch — since the mid-2010s).
If you don't have one, see [Linux: no systemd?](#linux-no-systemd).

By default, a `systemd --user` timer only runs while you're logged in — it stops the moment
your last session ends and doesn't survive a reboot into no session at all. On a headless
box, a server, or anything that reboots without an interactive login, also enable lingering
so the timer keeps running regardless:

```sh
loginctl enable-linger "$USER"
```

`petridish install` prints a reminder of this on Linux; it can't enable lingering for you
(it needs its own privileged `loginctl` call, not something to run silently on your behalf).

<details>
<summary id="installing-from-a-checkout-instead">Installing from a checkout instead</summary>

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
| `petridish` | Install, uninstall, health-check, and (macOS only) render the menu bar | macOS + Linux (`menubar` is macOS-only) |
| `swab` | The scanner. The **only** thing that writes `projects.json` | Cross-platform |
| `swab-hook` | The Claude Code hook. Appends one line to `events.ndjson`, nothing else | Cross-platform |
| `petri` | The terminal dashboard | Cross-platform |

```sh
petridish install       # wire up the daemon + the Claude Code hook (+ menu bar on macOS)
petridish uninstall     # remove all of that, leaving ~/.petridish intact
petridish doctor        # is the install intact?
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
install intact" (binaries resolve, the daemon registration points somewhere real — the
plist on macOS, the systemd timer + service unit on Linux — and every hook event is
registered).

### Uninstall semantics

`petridish uninstall` tears down the daemon registration (unloads the launchd job and
deletes its plist on macOS; disables and removes the systemd timer + service units on
Linux), and **structurally removes only the hook entries carrying the `# petridish`
marker** from `settings.json`. It does not restore the backup verbatim. That distinction
matters: if you or another tool edited `settings.json` after installing, a verbatim restore
would silently discard that edit.

`~/.petridish/` — config, state, the backup — is never deleted, so a later reinstall picks
up where you left off.

## Linux: no systemd?

`petridish install` (see [Install](#install) above) covers Linux the same way it covers
macOS, as long as a `systemd --user` session is available — which is the case on every
mainstream desktop/server distro. This section is only for the minority without one:
Alpine/OpenRC, Void/runit, Devuan, Gentoo's OpenRC default, WSL without `systemd=true`, or a
minimal container with no init at all. There, do by hand what `install` automates: build the
binaries, schedule the scan, register the Claude Code hook.

**1. Install the binaries** the same way as [Installing from a checkout
instead](#installing-from-a-checkout-instead) above — `petridish` here is only for `doctor`
in step 3; nothing on this path calls `install`/`uninstall`.

**2. Schedule `swab scan` to run every 60 seconds** with a cron entry. Wrap it in `flock` so
two ticks can never overlap and corrupt each other's state:

```
* * * * * flock -n /tmp/petridish-scan.lock $HOME/.cargo/bin/swab scan >> $HOME/.petridish/daemon.log 2>&1
```

Then register the Claude Code hook — this is what lets petridish sense when an agent is
running or waiting on you; without it, git-based facts (branch, dirty state) still work, but
agent activity won't show up. Add the following to `~/.claude/settings.json`, merging it
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

**3. Run it.**

```sh
petri             # the dashboard
swab doctor       # sanity-check config, roots, and state freshness
petridish doctor  # sanity-check the hook + binaries (the daemon check reports "missing" here,
                  # since `install` never ran — that's expected on the cron path)
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

### Testing the Linux systemd path for real

`cargo test`'s systemd coverage all goes through a `Systemctl` seam that records the argv
it's called with — real for the ordering/error-handling logic, but it never calls the real
`systemctl` binary or writes into a real unit search path. Two ways to exercise the real
thing:

**In CI**, on every push: `.github/workflows/ci.yml`'s `linux-systemd-smoke` job starts a
real `systemd --user` session on the `ubuntu-latest` runner (`loginctl enable-linger` plus
starting `user@<uid>.service`, since the runner has no active login session by default) and
runs `install` / `doctor` / `uninstall` against it for real.

**On your own Mac**, on demand: macOS has no `systemd --user` to test against, so this needs
a Linux VM. [Lima](https://lima-vm.io) (`brew install lima`) is the lightest way —
`limactl start default` gives you a real systemd VM whose default config mounts your home
directory at the same absolute path inside the guest, so `swab` scans your real project
roots instead of an empty test machine, and there's no code to copy over (the checkout is
the same files, just visible from both sides). Once the VM exists:

```sh
make lima-install    # build + a real `petridish install` (then go run `petri` yourself)
make lima-smoke      # full round trip: install -> real scan -> uninstall -> verify
make lima-uninstall  # uninstall, then verify every touchpoint
make lima-verify     # just re-run the post-uninstall checks
make lima-shell      # interactive shell in the VM, PATH set for the last build
```

See `petridish-cli/scripts/lima-dev.sh` for what each target does. In short: it builds with
`CARGO_TARGET_DIR` pointed outside the repo mount (which Lima mounts read-only, so `cargo
build`'s `target/` can't live inside it), then drives the real `petridish`/`swab`/`petri`
binaries inside the VM. Not part of `check`/`check-all` — it needs a VM, which CI doesn't
have.

## Docs

- `ARCHITECTURE.md` — architecture, empirical findings, the `projects.json` schema, and
  the distribution/installer requirements (§8). The authoritative reference.
- `petri/SPEC.md` — the dashboard's spec, authoritative for its screens and behaviour.
- `CONTRIBUTING.md` — what to run, and what must not break.
- `CLAUDE.md` — non-negotiable invariants for anyone changing this code.
- `ADR-0001`…`ADR-0004` — the decisions that are expensive to revisit.

## License

GPL-3.0-or-later — see `LICENSE`.
