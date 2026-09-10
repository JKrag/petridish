#!/usr/bin/env bash
#
# petri/scripts/flake-hunt.sh — measure how flaky the PTY suite actually is.
#
# `make check` answers "did the suite pass this time". That is the wrong
# question for a layer whose failures are probabilistic: a flaky test passes
# most of the time by definition, so a green run is not evidence of anything.
# This answers "how often does it fail, and which test", by running each PTY
# test binary many times at concurrency and counting.
#
# CONCURRENCY IS THE POINT, not extra load for its own sake. The races in this
# layer are between the test's wait and the child's repaint, and what perturbs
# that timing in practice is `cargo test` running many PTY sessions at once —
# which is exactly what `make check` does. Measured while diagnosing three of
# these: `s6_pty`'s enter-on-a-row failed 3 times in 48 at eight-way
# concurrency and 0 times in 25 under heavy plain CPU load. Spinning the CPU
# does not reproduce what this does.
#
# Usage:
#   petri/scripts/flake-hunt.sh [RUNS] [CONCURRENCY] [FILTER]
#
#   RUNS         total runs per test binary (default 24)
#   CONCURRENCY  how many run at once (default 8)
#   FILTER       only binaries whose name contains this (default: pty)
#
# Exits non-zero if anything failed, so it can gate a release if you want it to.
# It is deliberately NOT part of `make check`: it takes minutes, and a gate that
# slow gets skipped, which is how a suite ends up untrusted in the first place.
set -uo pipefail

RUNS=${1:-24}
CONCURRENCY=${2:-8}
FILTER=${3:-pty}

# Validate before doing minutes of work, because both numbers fail SILENTLY rather than
# loudly. `CONCURRENCY=0` makes the inner batch loop launch nothing, so `i` never advances
# and the outer loop spins forever. `RUNS=0` divides by zero in the percentage, and any
# non-numeric value makes `[ "$i" -lt "$RUNS" ]` an arithmetic error on every iteration.
for pair in "RUNS:$RUNS" "CONCURRENCY:$CONCURRENCY"; do
  name=${pair%%:*}
  value=${pair#*:}
  case "$value" in
    *[!0-9]* | "")
      echo "FAIL: $name must be a positive integer, got '$value'."
      exit 2
      ;;
  esac
  if [ "$value" -lt 1 ]; then
    echo "FAIL: $name must be at least 1, got '$value'."
    exit 2
  fi
done

cd "$(dirname "$0")/../.." || exit 1

echo "building test binaries…"
if ! cargo test -p petri --no-run >/dev/null 2>&1; then
  echo "FAIL: the test binaries do not build — fix that before measuring flakiness."
  exit 2
fi

# Ask cargo where the binaries are rather than globbing target/debug/deps, which
# accumulates stale copies from previous builds and would happily measure one.
#
# Extracted with sed rather than python3/jq: CLAUDE.md's one-toolchain rule means a
# contributor with the documented Rust toolchain and nothing else must be able to run
# `make flake-hunt`, and a `python3` in the pipeline quietly made this the one command that
# needed a second one. Only `"executable"` is pulled out — cargo emits it as `null` (no
# quotes) on the artifact lines that are not test binaries, so the quoted-value pattern
# skips those without needing to parse the JSON. The target name is then the basename minus
# cargo's `-<hash>` suffix, which is the same string the JSON's `target.name` carried.
BINS=$(cargo test -p petri --no-run --message-format=json 2>/dev/null \
  | sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' \
  | while IFS= read -r exe; do
      base=$(basename "$exe")
      printf '%s\t%s\n' "$(echo "$base" | sed 's/-[0-9a-f]\{7,\}$//')" "$exe"
    done \
  | sort -u)

if [ -z "$BINS" ]; then
  echo "FAIL: could not locate any test binaries."
  exit 2
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

total_runs=0
total_fail=0
flaky_names=""

while IFS=$'\t' read -r name exe; do
  case "$name" in
    *"$FILTER"*) ;;
    *) continue ;;
  esac

  # A directory per binary rather than a `code_${name}_*` glob in one flat dir: that glob
  # also matches a longer binary name having this one as a prefix, which would silently
  # attribute one binary's failures to another the day such a pair is added.
  mkdir -p "$tmp/$name"

  fails=0
  i=0
  while [ "$i" -lt "$RUNS" ]; do
    batch=0
    while [ "$batch" -lt "$CONCURRENCY" ] && [ "$i" -lt "$RUNS" ]; do
      i=$((i + 1))
      batch=$((batch + 1))
      ( "$exe" --test-threads=1 >"$tmp/$name/out_$i" 2>&1; echo $? >"$tmp/$name/code_$i" ) &
    done
    wait
  done

  for f in "$tmp/$name"/code_*; do
    [ "$(cat "$f")" = "0" ] || fails=$((fails + 1))
  done

  total_runs=$((total_runs + RUNS))
  total_fail=$((total_fail + fails))

  if [ "$fails" -gt 0 ]; then
    pct=$(awk "BEGIN { printf \"%.1f\", 100 * $fails / $RUNS }")
    printf '  %-24s %3d/%-3d failed  (%s%%)\n' "$name" "$fails" "$RUNS" "$pct"
    flaky_names="$flaky_names $name"
    # The first failing run's output, so a hunt is a diagnosis and not just a
    # number — most of the cost here is getting the failure to happen at all.
    for f in "$tmp/$name"/code_*; do
      if [ "$(cat "$f")" != "0" ]; then
        echo "    first failure:"
        grep -E "panicked at|assertion|hang, not a slow pass" "${f/code_/out_}" \
          | head -3 | sed 's/^/      /'
        break
      fi
    done
  else
    printf '  %-24s %3d/%-3d failed\n' "$name" 0 "$RUNS"
  fi
done <<< "$BINS"

echo
# Before the clean/flaky branch, because zero runs is neither. A mistyped FILTER matches no
# binary, every loop above is skipped, and `total_fail -eq 0` would then print
# "clean: 0 failures in 0 runs" and exit 0 — a gate reporting success for having measured
# nothing at all, which is worse than no gate.
if [ "$total_runs" -eq 0 ]; then
  echo "FAIL: no test binary matched FILTER '$FILTER' — nothing was measured."
  exit 2
fi

if [ "$total_fail" -eq 0 ]; then
  echo "clean: 0 failures in $total_runs runs at concurrency $CONCURRENCY"
  exit 0
fi

rate=$(awk "BEGIN { printf \"%.2f\", 100 * $total_fail / $total_runs }")
echo "FLAKY: $total_fail failures in $total_runs runs ($rate%) at concurrency $CONCURRENCY"
echo "flaky binaries:$flaky_names"
exit 1
