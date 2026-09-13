# petridish — one entry point for the whole gate.
#
# `make check` is the command to run before proposing any change. It is what CI
# runs, so a green `make check` locally means a green CI run.
#
# Note the form of the `check` target: its gates are PREREQUISITES, not commands
# joined by `;` in one recipe line. That is load-bearing. A recipe like
#     check:
#         cargo fmt --check; cargo test
# returns only the LAST command's exit status, so a formatting failure would
# report success. Verified empirically — keep them as prerequisites.

.PHONY: help fmt fmt-check clippy test deny msrv raycast check check-all clean flake-hunt \
	lima-install lima-uninstall lima-verify lima-smoke lima-shell

.DEFAULT_GOAL := help

help:           ## Show this help.
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| sed -e 's/:.*## /|/' \
		| awk -F'|' '{ printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2 }'

fmt:            ## Reformat the workspace.
	cargo fmt --all

fmt-check:      ## Fail if anything is unformatted.
	cargo fmt --all --check

clippy:         ## Lint, warnings are errors.
	cargo clippy --workspace --all-targets --all-features -- -D warnings

test:           ## Run the Rust workspace tests.
	cargo test --locked --workspace

# Deliberately NOT a prerequisite of `check` or `check-all`: it takes minutes,
# and a gate that slow gets skipped — which is how a suite stops being trusted.
# Run it before a release, or when a PTY test fails once and you want to know
# whether that meant anything.
#
# Args: RUNS, CONC, FILTER — e.g. `make flake-hunt RUNS=48 FILTER=s8_pty`. All three are
# passed explicitly, defaults included, because the script's arguments are POSITIONAL: a
# bare `$(RUNS) $(CONC)` expands to nothing at all when unset, so `make flake-hunt CONC=4`
# would hand the script a single argument and it would read that 4 as RUNS. `$(or ...)`
# keeps each position filled whatever the caller sets. FILTER was documented here before it
# was ever forwarded; it is now.
flake-hunt:     ## Measure PTY test flakiness (slow; RUNS=24 CONC=8 FILTER=pty by default).
	petri/scripts/flake-hunt.sh "$(or $(RUNS),24)" "$(or $(CONC),8)" "$(or $(FILTER),pty)"

deny:           ## Licence + advisory audit (needs `cargo install cargo-deny`).
	cargo deny check licenses advisories

msrv:           ## Build on the declared rust-version floor (needs that toolchain).
	cargo +$(shell grep '^rust-version' Cargo.toml | head -1 | sed 's/.*= *"//;s/".*//') \
		check --locked --workspace --all-targets

# Uses whatever is already in node_modules rather than `npm ci`. CI does the
# clean install; locally, reinstalling from scratch on every check is slow and
# fails outright if the npm cache has permission problems, which is not a
# signal about this code. Run `npm ci` in integrations/raycast yourself if the
# lockfile changed.
raycast:        ## Check the Raycast extension (needs node; run `npm ci` there first).
	cd integrations/raycast && ./node_modules/.bin/tsc --noEmit && npm test \
		&& ./node_modules/.bin/eslint . \
		&& ./node_modules/.bin/prettier --check "src/**/*.{ts,tsx}" "tests/**/*.ts"

# The everyday gate: everything that needs nothing but a Rust toolchain.
check: fmt-check clippy test   ## Fast gate: formatting + lints + tests.

# Everything CI runs. Kept separate because `deny`, `msrv` and `raycast` each
# need a tool a contributor may not have installed — cargo-deny, a second
# toolchain, and node respectively — and a gate that fails on a missing tool
# trains people to ignore it. Run this before opening a PR; run `check` while
# iterating.
check-all: check deny msrv raycast   ## Everything CI runs.

clean:          ## Remove build output.
	cargo clean

# `cargo test`'s systemd coverage all goes through the RecordingSystemctl seam
# (real for argv/ordering/error-mapping, but never the real `systemctl`
# binary or a real unit search path). These exercise the real thing, inside a
# Lima VM (LIMA_INSTANCE=default by default), against your own real project
# data — macOS has no systemd --user to test this against directly. See
# petridish-cli/scripts/lima-dev.sh for the one-time VM setup these assume.
# Deliberately not part of `check`/`check-all`: they need a Lima VM, which CI
# doesn't have (CI's own Linux systemd coverage is the
# linux-systemd-smoke job in .github/workflows/ci.yml instead).
lima-install:   ## Build + real `petridish install` in the Lima VM (for running `petri` yourself).
	petridish-cli/scripts/lima-dev.sh install

lima-uninstall: ## `petridish uninstall` in the Lima VM, then verify every touchpoint.
	petridish-cli/scripts/lima-dev.sh uninstall

lima-verify:    ## Re-run just the post-uninstall touchpoint checks in the Lima VM.
	petridish-cli/scripts/lima-dev.sh verify

lima-smoke:     ## Full install -> real scan -> uninstall round trip in the Lima VM.
	petridish-cli/scripts/lima-dev.sh smoke

lima-shell:     ## Interactive shell in the Lima VM, PATH set for the last build (run `petri` here).
	petridish-cli/scripts/lima-dev.sh shell
