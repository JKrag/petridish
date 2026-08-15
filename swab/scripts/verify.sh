#!/usr/bin/env bash
# The per-round mechanical gate. Protected file — do not edit as part of an AFK round.
#
# Usage: verify.sh <file:test-filter> [file:test-filter ...]
# e.g.:  verify.sh src/schema.rs:schema::tests
#        verify.sh src/sensors/copilot.rs:sensors::copilot::tests src/sensors/quota.rs:sensors::quota::tests
#
# Centralizes two things every round needs and easy to get subtly wrong by hand:
#   1. cargo is NOT on the default PATH in this environment (no ~/.cargo/bin; the toolchain
#      lives under ~/.rustup/toolchains/.../bin) — this script puts it on PATH itself so it
#      works the same whether run by the delegated model or the orchestrator.
#   2. The protected-files check: nothing outside swab/ may differ from BASE, and no
#      swab file outside this round's declared target(s) may differ either. This turns
#      "leave everything else alone" from an instruction into a gate. Set BASE_SHA in the
#      environment to enable it (the orchestrator sets this; if unset, this check is skipped
#      with a warning — expected when a delegated model runs this mid-round to self-check).
set -euo pipefail

if [ "$#" -lt 1 ]; then
  echo "usage: verify.sh <file:test-filter> [file:test-filter ...]" >&2
  exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SWAB_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$SWAB_DIR/.." && pwd)"

export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo still not found on PATH after adding the rustup toolchain bin dir" >&2
  exit 2
fi

TARGET_FILES=()
TEST_FILTERS=()
for pair in "$@"; do
  TARGET_FILES+=("${pair%%:*}")
  TEST_FILTERS+=("${pair#*:}")
done

cd "$SWAB_DIR"

echo "== no leftover todo!() ==" >&2
for f in "${TARGET_FILES[@]}"; do
  if grep -q 'todo!' "$f"; then
    echo "FAIL: $f still contains todo!()" >&2
    exit 1
  fi
done

echo "== cargo build --all-targets ==" >&2
cargo build --all-targets

for filter in "${TEST_FILTERS[@]}"; do
  echo "== cargo test $filter ==" >&2
  TEST_LOG="$(mktemp)"
  cargo test "$filter" -- --test-threads=1 2>&1 | tee "$TEST_LOG"
  if ! grep -qE '^test result: ok\. [1-9][0-9]* passed' "$TEST_LOG"; then
    echo "FAIL: no nonzero passing test count matched filter '$filter' — an empty match is NOT a pass" >&2
    rm -f "$TEST_LOG"
    exit 1
  fi
  rm -f "$TEST_LOG"
done

echo "== protected-files check (nothing outside the declared target(s) changed) ==" >&2
cd "$REPO_ROOT"
if [ -n "${BASE_SHA:-}" ]; then
  CHANGED_OUTSIDE_SWAB="$(git diff --name-only "$BASE_SHA" -- . ':!swab' 2>/dev/null || true)"
  UNTRACKED_OUTSIDE_SWAB="$(git status --porcelain -- . ':!swab' 2>/dev/null || true)"
  if [ -n "$CHANGED_OUTSIDE_SWAB" ] || [ -n "$UNTRACKED_OUTSIDE_SWAB" ]; then
    echo "FAIL: changes detected outside swab/ — this round must only touch its declared target file(s)" >&2
    echo "$CHANGED_OUTSIDE_SWAB" >&2
    echo "$UNTRACKED_OUTSIDE_SWAB" >&2
    exit 1
  fi
  CHANGED_IN_SWAB="$(git diff --name-only "$BASE_SHA" -- swab; git status --porcelain -- swab | awk '{print $2}')"
  for f in $CHANGED_IN_SWAB; do
    rel="${f#swab/}"
    is_target=0
    for t in "${TARGET_FILES[@]}"; do
      [ "$rel" = "$t" ] && is_target=1
    done
    if [ "$is_target" -eq 0 ]; then
      echo "FAIL: $f changed but is not one of this round's declared target files (${TARGET_FILES[*]})" >&2
      exit 1
    fi
  done
else
  echo "SKIP: BASE_SHA not set in environment — protected-files check not run (orchestrator sets this; if you are the delegated model running this manually mid-round, that's expected)" >&2
fi

echo "ALL CHECKS PASSED"
