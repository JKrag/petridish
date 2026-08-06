#!/bin/bash
# Install (or --uninstall) the petridish launchd job and Claude Code hook.
#
# All the logic that matters — structural settings.json edits, plist
# templating, launchd bootstrap/bootout — lives in src/petridish/installer.py,
# where it's unit-tested against real tmpdir fixtures. This script's only job
# is finding a python3 that can actually `import petridish`: the ambient
# `python3` on PATH is very unlikely to be it (uv tool / pipx install into an
# isolated venv and only put the console-script shims on PATH), so we resolve
# the shim's own interpreter instead — same trick as `command -v swab-hook`
# performs for the hook path inside installer.py (D1: never hardcode).
set -euo pipefail

if ! command -v swab >/dev/null 2>&1; then
    echo "error: 'swab' not found on PATH. Install it first:" >&2
    echo "  uv tool install --editable ." >&2
    exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "error: python3 not found on PATH" >&2
    exit 1
fi

SWAB_REAL="$(python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "$(command -v swab)")"
VENV_PY="$(dirname "$SWAB_REAL")/python3"

if [[ -x "$VENV_PY" ]]; then
    exec "$VENV_PY" -m petridish.installer "$@"
fi

# Fallback: swab wasn't installed into an isolated venv (e.g. plain
# PYTHONPATH=src dev setup) — the ambient python3 is the right one.
exec python3 -m petridish.installer "$@"
