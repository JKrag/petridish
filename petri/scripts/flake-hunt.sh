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

cd "$(dirname "$0")/../.." || exit 1

echo "building test binaries…"
if ! cargo test -p petri --no-run >/dev/null 2>&1; then
  echo "FAIL: the test binaries do not build — fix that before measuring flakiness."
  exit 2
fi

# Ask cargo where the binaries are rather than globbing target/debug/deps, which
# accumulates stale copies from previous builds and would happily measure one.
BINS=$(cargo test -p petri --no-run --message-format=json 2>/dev/null \
  | python3 -c '
import json, sys
for line in sys.stdin:
    try:
        m = json.loads(line)
    except ValueError:
        continue
    exe = m.get("executable")
    if not exe:
        continue
    name = m.get("target", {}).get("name", "")
    print(f"{name}\t{exe}")
' | sort -u)

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

  fails=0
  i=0
  while [ "$i" -lt "$RUNS" ]; do
    batch=0
    while [ "$batch" -lt "$CONCURRENCY" ] && [ "$i" -lt "$RUNS" ]; do
      i=$((i + 1))
      batch=$((batch + 1))
      ( "$exe" --test-threads=1 >"$tmp/out_${name}_$i" 2>&1; echo $? >"$tmp/code_${name}_$i" ) &
    done
    wait
  done

  for f in "$tmp"/code_"${name}"_*; do
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
    for f in "$tmp"/code_"${name}"_*; do
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
if [ "$total_fail" -eq 0 ]; then
  echo "clean: 0 failures in $total_runs runs at concurrency $CONCURRENCY"
  exit 0
fi

rate=$(awk "BEGIN { printf \"%.2f\", 100 * $total_fail / $total_runs }")
echo "FLAKY: $total_fail failures in $total_runs runs ($rate%) at concurrency $CONCURRENCY"
echo "flaky binaries:$flaky_names"
exit 1
