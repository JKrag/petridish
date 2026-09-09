#!/usr/bin/env bash
# petri/scripts/afk-verify.sh — prints the number of failing tests in `petri`.
#
# The global verify command for the focus-panel job (petri/PLAN-focus-panel.md §2).
# The metric is "failing tests in petri, lower is better, 0 required to pass", which
# makes each round a ratchet: a round either reduces the score or gets reverted.
#
# Prints a single integer on stdout and always exits 0, so the AFK loop's
# "verify command errored" escalation stays reserved for a genuinely broken
# environment (no cargo, no toolchain) rather than firing on a bad round.
#
# TWO separate build guards, because there are two distinct ways a build
# failure can make a bad round score as a good one:
#
#  1. Nothing builds at all -> zero "test result:" lines -> awk sums nothing
#     and prints 0, a perfect score for a round that broke everything.
#  2. *One* integration target fails to compile. `cargo test -p petri` builds
#     each file under tests/ as its own binary, so the others still run and
#     still print "test result:". The failing target's tests simply stop being
#     counted, and the score DROPS. This is the nastier of the two: it looks
#     like progress, and deleting a test file has the same signature.
#
# Guard 1 is `--no-run` (compiles every target, runs nothing). Guard 2 is the
# binary count, pinned at the end of each scaffold phase.
set -uo pipefail

# Pinned by the scaffold phase: how many test binaries must report a result.
# Update it in the same commit that adds or removes a test file, never in a
# commit that is trying to move the score.
#
# 32 as of Phase A, 34 after Phase C, 36 after the attended PTY pass added
# s11_pty_focus.rs and s12_pty_mini.rs. Derived empirically by running the
# `grep -c` below, NOT by counting the files under tests/: `cargo test -p petri`
# also reports for the lib target's inline `#[cfg(test)]` modules and for the
# doc-test pass, so a file count is off by several and would fire guard 2 on
# every round.
EXPECTED_BINARIES=${EXPECTED_BINARIES:-36}

if ! cargo test -p petri --no-run >/dev/null 2>&1; then
  echo 9999            # guard 1: something does not compile
  exit 0
fi

out=$(cargo test -p petri --no-fail-fast 2>&1)
seen=$(grep -c '^test result:' <<<"$out")

if [ "$seen" -lt "$EXPECTED_BINARIES" ]; then
  echo 9999            # guard 2: a target vanished from the run
  exit 0
fi

grep '^test result:' <<<"$out" \
  | awk '{for (i = 1; i <= NF; i++) if ($i == "failed;") s += $(i-1)} END {print s+0}'
