#!/usr/bin/env bash
# The differential oracle: runs Python swab and Rust swab-rs against the same fixture
# $HOME and diffs their projects.json output (nondeterministic fields masked). This is the
# correctness gate the AFK loop's verify commands build toward — see swab-rs/README or
# .afk/program-rustport.md for how each module's verify step uses it.
#
# Usage: swab-rs/scripts/diff_check.sh [fixture-dir]
# Exit 0 on match, 1 on mismatch or a runtime error from either side.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SWAB_RS_DIR="$REPO_ROOT/swab-rs"

FIXTURE_ROOT="${1:-${TMPDIR:-/tmp}/swab-rs-fixture-$$}"
rm -rf "$FIXTURE_ROOT"
mkdir -p "$FIXTURE_ROOT"

echo "Building fixture $HOME under $FIXTURE_ROOT ..." >&2
FIXTURE_HOME="$(python3 "$SCRIPT_DIR/make_fixture_home.py" "$FIXTURE_ROOT")"
echo "Fixture HOME: $FIXTURE_HOME" >&2

PY_OUT="$FIXTURE_ROOT/py-projects.json"
RS_OUT="$FIXTURE_ROOT/rs-projects.json"

echo "Running Python swab scan ..." >&2
HOME="$FIXTURE_HOME" "$REPO_ROOT/.venv/bin/swab" --state "$PY_OUT" scan >&2

echo "Building swab-rs (debug) ..." >&2
if command -v cargo >/dev/null 2>&1; then
  CARGO_BIN=cargo
elif [ -x "$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo" ]; then
  CARGO_BIN="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo"
else
  echo "cargo not found on PATH or under ~/.rustup/toolchains/stable-aarch64-apple-darwin/bin" >&2
  exit 1
fi

( cd "$SWAB_RS_DIR" && "$CARGO_BIN" build --bin swab-rs >&2 )

echo "Running Rust swab-rs scan ..." >&2
HOME="$FIXTURE_HOME" "$SWAB_RS_DIR/target/debug/swab-rs" scan --state "$RS_OUT" >&2

echo "Comparing output ..." >&2
python3 "$SCRIPT_DIR/compare_radar.py" "$PY_OUT" "$RS_OUT"
