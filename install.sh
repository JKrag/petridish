#!/bin/bash
# Install (or --uninstall) the petridish launchd job and Claude Code hook.
#
# All the logic that matters — structural settings.json edits, plist
# templating, launchd bootstrap/bootout — lives in src/petridish/installer.py,
# where it's unit-tested against real tmpdir fixtures. This script's only job
# is finding a python3 that can actually `import petridish`: the ambient
# `python3` on PATH is very unlikely to be it (uv tool / pipx install into an
# isolated venv and only put the console-script shims on PATH), so we resolve
# a shim's own interpreter instead.
#
# That shim used to be `swab` itself, back when it was a Python console
# script. Now `swab`/`swab-hook` are the Rust binaries (swab/) that fully
# replaced src/petridish/{cli,hook}.py — they're not Python at all, so their
# own realpath can't point at a venv's python3 anymore. `petri` (the TUI,
# still Python, still installed the same `uv tool install --editable .` way)
# is the shim we resolve against instead — same trick, different anchor.
# installer.py's own `command -v swab-hook` (D1: never hardcode) is unrelated
# to this and still resolves the Rust hook binary correctly on its own.
set -euo pipefail

if ! command -v swab >/dev/null 2>&1 || ! command -v swab-hook >/dev/null 2>&1; then
    echo "error: 'swab'/'swab-hook' not found on PATH. Build and install them first:" >&2
    echo "  cargo install --path swab" >&2
    exit 1
fi

if ! command -v petri >/dev/null 2>&1; then
    echo "error: 'petri' not found on PATH. Install it first:" >&2
    echo "  uv tool install --editable ." >&2
    exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "error: python3 not found on PATH" >&2
    exit 1
fi

PETRI_REAL="$(python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "$(command -v petri)")"
VENV_PY="$(dirname "$PETRI_REAL")/python3"

if [[ -x "$VENV_PY" ]]; then
    exec "$VENV_PY" -m petridish.installer "$@"
fi

# Fallback: petri wasn't installed into an isolated venv (e.g. plain
# PYTHONPATH=src dev setup) — the ambient python3 is the right one.
exec python3 -m petridish.installer "$@"
