# petridish — one entry point for the whole gate.
#
# `make check` is the command to run before proposing any change. It is what CI
# runs, so a green `make check` locally means a green CI run.
#
# Note the form of the `check` target: its gates are PREREQUISITES, not commands
# joined by `;` in one recipe line. That is load-bearing. A recipe like
#     check:
#         pytest -q; pyright
# returns only the LAST command's exit status, so failing tests would report
# success. Verified empirically — keep them as prerequisites.

.PHONY: help install test typecheck pyver check clean

.DEFAULT_GOAL := help

help:           ## Show this help.
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| sed -e 's/:.*## /|/' \
		| awk -F'|' '{ printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2 }'

install:        ## Install the package + dev extras into a uv-managed venv.
	uv sync --extra dev

test:           ## Run the test suite (pytest).
	uv run --extra dev pytest -q

typecheck:      ## Fail if pyright errors under src/ rose above the baseline.
	uv run --extra dev python3 scripts/typecheck_ratchet.py

pyver:          ## Fail if src/ uses stdlib APIs newer than requires-python.
	uv run --extra dev python3 scripts/check_pyver.py

check: test typecheck pyver   ## Full gate: tests + typecheck + version floor.

clean:          ## Remove caches (leaves .venv alone — use `rm -rf .venv` for that).
	find . -type d -name __pycache__ -prune -exec rm -rf {} +
	rm -rf .pytest_cache .ruff_cache dist build *.egg-info
