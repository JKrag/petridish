#!/usr/bin/env bash
# Per-function parity oracle for modules R2-R7 (schema/discovery/git/events/sensors),
# where the aggregator (R8) isn't wired up yet so diff_check.sh's whole-scan comparison
# can't run. Runs the same JSON args through py_probe.py (ground truth: the real Python
# implementation) and examples/probe.rs (the port), then diffs the canonicalized JSON.
#
# Usage: parity_check.sh <function> '<json-args>'
# e.g.:  parity_check.sh resolve_root '{"cwd": "/tmp/x/packages/core", "roots": ["/tmp/x"]}'
#
# Exit 0 on match, 1 on mismatch, 2 on usage/build error.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: parity_check.sh <function> '<json-args>'" >&2
  exit 2
fi
FUNCTION="$1"
ARGS_JSON="$2"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SWAB_DIR="$REPO_ROOT/swab"

if command -v cargo >/dev/null 2>&1; then
  CARGO_BIN=cargo
elif [ -x "$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo" ]; then
  CARGO_BIN="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo"
else
  echo "cargo not found on PATH or under ~/.rustup/toolchains/stable-aarch64-apple-darwin/bin" >&2
  exit 2
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

PY_OUT="$TMP_DIR/py.json"
RS_OUT="$TMP_DIR/rs.json"

# events_read_and_compact TRUNCATES its input file as a side effect (matching the real
# function's "consume exactly once" contract) — back up and restore the referenced file's
# path between the two probe runs, or the second (Rust) run sees an empty file and a
# mismatch is masked as a false "match" on two empty results.
MUTATED_PATH=""
if [ "$FUNCTION" = "events_read_and_compact" ]; then
  MUTATED_PATH="$(echo "$ARGS_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin).get('path',''))")"
  if [ -n "$MUTATED_PATH" ] && [ -f "$MUTATED_PATH" ]; then
    cp "$MUTATED_PATH" "$TMP_DIR/events_backup.ndjson"
  fi
fi

echo "$ARGS_JSON" | python3 "$SCRIPT_DIR/py_probe.py" "$FUNCTION" > "$PY_OUT"

if [ -n "$MUTATED_PATH" ] && [ -f "$TMP_DIR/events_backup.ndjson" ]; then
  cp "$TMP_DIR/events_backup.ndjson" "$MUTATED_PATH"
fi

( cd "$SWAB_DIR" && "$CARGO_BIN" build --example probe --quiet )
echo "$ARGS_JSON" | "$SWAB_DIR/target/debug/examples/probe" "$FUNCTION" > "$RS_OUT"

# Canonicalize (sorted keys) before comparing — HashMap-backed Rust output has no
# guaranteed key order, and dict order isn't semantically meaningful here either.
PY_CANON="$(python3 -c "import json,sys; print(json.dumps(json.load(open('$PY_OUT')), sort_keys=True))")"
RS_CANON="$(python3 -c "import json,sys; print(json.dumps(json.load(open('$RS_OUT')), sort_keys=True))")"

if [ "$PY_CANON" = "$RS_CANON" ]; then
  echo "MATCH"
  exit 0
else
  echo "MISMATCH for $FUNCTION($ARGS_JSON):" >&2
  echo "  python: $PY_CANON" >&2
  echo "  rust:   $RS_CANON" >&2
  exit 1
fi
