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

.PHONY: help fmt fmt-check clippy test deny msrv raycast check check-all clean

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

# `--test-threads=1` is required, and the reason is NOT the one this repo used
# to give. It is not a Python-side artifact — the Python is gone and the need
# remains. Three tests in `swab/src/cli.rs` mutate the `HOME` environment
# variable to point at a scratch directory, and env vars are per-process, so in
# parallel they race with every other test that reads `HOME`. Measured: running
# `cargo test -p swab --lib` without this flag fails 3 runs out of 3, on
# `doctor_fails_when_the_hook_is_registered_on_only_some_events` among others.
# Removing the flag needs those three tests to stop touching the environment
# first.
test:           ## Run the Rust workspace tests.
	cargo test --locked --workspace -- --test-threads=1

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
