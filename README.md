# petridish

Local monitoring daemon for macOS: crawls project roots, tracks git state, senses AI agent
activity, and aggregates into `~/.petridish/projects.json`.

## Install (dev)

Two separate pieces, two separate toolchains:

**`swab` / `swab-hook`** (the scanner) are Rust, built from `swab-rs/`:

```sh
cargo install --path swab-rs
```

This puts `swab` and `swab-hook` on `~/.cargo/bin` (verify it's on `PATH`). Verify:

```sh
swab --help
```

**`petri`** (the TUI dashboard) is still Python — it only ever reads `~/.petridish/projects.json`
(via `petridish.schema`), it never scans:

```sh
uv tool install --editable .
```

This puts a `petri` shim on `~/.local/bin` (already first on `PATH` for most `uv` setups).

## Wire up the launchd job + Claude Code hook

```sh
./install.sh              # install: launchd job (60s tick) + Claude Code PreToolUse/Stop hook
./install.sh --uninstall  # remove both, cleanly
```

`install.sh` requires `swab`/`swab-hook` and `petri` to already be on `PATH` (see above). It:

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
changes (e.g. after `cargo install --path swab-rs` relocates it), run
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

## Config

`~/.petridish/config.toml` — entirely optional; every field has a default. Run
`swab config` for the full field reference (sourced from `swab-rs/src/config.rs`'s own
`Config::default()`, so it can't drift out of sync with the code) and an example.
`install.sh` writes a commented-out template there on first install (see
`DEFAULT_CONFIG_TOML` in `src/petridish/installer.py`).

## Docs

- `ARCHITECTURE.md` — language-agnostic architecture, empirical findings, and the
  `projects.json` schema; the current authoritative reference
- `DESIGN.md` — original system design document (superseded by `ARCHITECTURE.md` where the
  two disagree, kept as historical context)
- `docs/archive/IMPLEMENTATION_PLAN.md` — the original all-Python build spec, archived once
  the scanner was ported to Rust
- `CLAUDE.md` — non-negotiable invariants for anyone changing this code

## License

GPL-3.0-or-later — see `LICENSE`.
