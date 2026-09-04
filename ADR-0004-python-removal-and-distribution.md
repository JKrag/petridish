# Removing Python, and how petridish is distributed

**Status:** accepted

## Context

The repo carried two toolchains. Rust owned everything that wrote `projects.json`
(`swab`) and the dashboard (`petri`); Python owned the read-side — `schema.py` as a
parsing contract, `petripy` as the deprecated original TUI, `menubar.py` for xbar, and
`installer.py`, which wired up the launchd job and the Claude Code hook.

That split had a reason when the scanner was mid-port. It stopped having one once `petri`
was built and trusted. What remained was a Python package whose only load-bearing member
was the installer, plus documentation that kept sending readers toward `petripy` when
they wanted `petri`.

Making the project installable by a stranger forced the question. The intended
distribution channel is a Homebrew tap: `brew install`, then one command to wire things
up. Homebrew can ship the binaries, but nothing in that pipeline writes a launchd plist
or edits `~/.claude/settings.json` — `installer.py` did. So a Python-free install was a
prerequisite for a usable release, not a tidiness exercise that could follow one.

## Decision

**Delete the Python read-side entirely.** Port `installer.py` and `menubar.py` to Rust;
retire `petripy`, whose replacement has been shipping for some time; drop `schema.py`,
whose contract `petridish-core` already expresses for every remaining reader.

**Put the ported code in a new crate, `petridish-cli`, producing a binary named
`petridish`.** Not a third `[[bin]]` in `swab`: ADR-0002 established that `swab-hook` is
the declared latency path and must not carry unrelated dependency weight, and the
argument transfers unchanged. It also makes the boundary structural — `petridish-cli`
does not depend on `swab`, so it cannot reach the state-file writer.

`petridish` is the name a new user types first, which is why the installer owns it rather
than `swab`. `swab install` would have meant nothing to someone reading the README.

**Distribute via a Homebrew tap, with a shell installer and `cargo install` as
alternatives.** Not PyPI, which ARCHITECTURE.md §8.2 previously named as the primary
channel and which has nothing left to publish.

**`petridish install` owns launchd, not Homebrew's `service` block.** A brew-managed
service would give brew users a slightly shorter path and everyone else none at all, and
requirements D1/D2 (resolve the binary path at install time, assume no `PATH` in
non-interactive contexts) would then hold for one install route and not the other. One
install story, exercised by every user, is worth more than a marginally shorter one for
some.

## Consequences

**The crate name and the binary name differ.** `petridish` is taken on crates.io by an
unrelated, actively-published project-scaffolding tool, as are the bare `petri` and
`swab`. `petri-dish` is free and normalises to `petri_dish`, which does not collide. A
crate may ship a binary under any name, so the crate publishes as `petri-dish` while
everything a user sees stays `petridish` — matching `~/.petridish/`,
`com.petridish.daemon`, the `# petridish` hook marker, and the Homebrew formula. The
crates.io publish itself is deferred until after the beta; the name is the point of
claiming it.

**D1–D6 survive, but two needed reinterpreting in place** (the D-numbers are cited from
code, so they are frozen identifiers rather than prose):

- **D3** said runtime dependencies stay at zero, with `src/petridish/` as its literal
  subject and Homebrew's Python-formula `resource` blocks as its supporting argument.
  Both are gone. It now reads as a constraint on the CLI crate's dependency tree, which
  is `clap`, `serde_json`, `chrono` and `petridish-core` — no async runtime, no HTTP
  client, no plist library. PATH lookup, the scratch-directory test helper and the
  `getuid` call are each a few lines of `std` rather than a dependency.
- **D5** (declare macOS-only, fail early) survives as a requirement, but both cited
  implementations disappeared with the pyproject classifier and `installer.py`'s
  `check_platform`. It now cites `petridish-cli`. Note that `check_platform` takes the OS
  as a *parameter* rather than being a `#[cfg(target_os)]` gate — CI compiles this crate
  on Linux, and a compile-time gate would make its tests unrunnable there.

**The xbar plugin is generated, not shipped as a file.** It has to embed an absolute path
to the binary. xbar is launched by the GUI session, inherits launchd's environment rather
than a shell's, and sources no rc file, so neither `~/.cargo/bin` nor `/opt/homebrew/bin`
is reliably on its `PATH`. This is the same constraint that made the Python plugin
substitute an absolute interpreter path into its shebang. A hand-written plugin calling
`petridish menubar` works when tested in a terminal and shows nothing in the menu bar.

**Binary paths are absolutised but not canonicalised.** Under Homebrew,
`/opt/homebrew/bin/swab` is a symlink into a version-stamped Cellar directory; resolving
it would bake that version into the launchd plist and the next `brew upgrade` would leave
the daemon pointing at a path that no longer exists. `petridish doctor` checks for
exactly this: it reads the program path back out of the installed plist and verifies it
still exists.

**ADR-0003's differential oracle is gone, and had already stopped working.** It described
diffing Rust against the Python implementation; `swab/scripts/py_probe.py` imported
modules deleted long before this change, so the scripts could not have run. The
replacement for the menubar port was a one-off differential check performed *before* the
deletion — `petridish menubar` was verified byte-identical to `render_menubar` on all four
committed fixtures — plus ported unit tests. For the schema, `petridish-core` now carries
the golden-fixture and timestamp-format tests that lived in `test_schema.py`.

**`--test-threads=1` is still required, for a reason the docs had wrong.** CLAUDE.md
attributed it to Python-side fixtures mutating `HOME`. The Python is gone and the
requirement remains: three tests in `swab/src/cli.rs` mutate `$HOME`, which is
process-global. Fixing those is worthwhile and is deliberately out of scope here, since
`swab` is frozen for this work package.

## Alternatives considered

**Keep a stdlib-only Python xbar plugin as an exception.** xbar plugins are user-space
scripts, so this is defensible. Rejected because `menubar.py` was 128 lines of pure render
logic over types `petridish-core` already models — the port cost less than the permanent
cost of a second toolchain in the repo, in CI, and in the install instructions.

**Rename the whole project to something free on crates.io.** Cheapest at this moment, and
genuinely considered. Rejected because `~/.petridish/`, `com.petridish.daemon` and the
`# petridish` hook marker are load-bearing identifiers already on users' machines, and
because the name is only contested on the one channel that is optional.
