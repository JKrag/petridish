#!/usr/bin/env bash
#
# petridish-cli/scripts/lima-dev.sh — drive petridish's real systemd --user
# install/uninstall path inside a Lima VM, from macOS.
#
# `cargo test`'s systemd coverage all goes through the RecordingSystemctl
# seam, and .github/workflows/ci.yml's linux-systemd-smoke job covers the
# real thing but only in CI. This is the same exercise on demand, against
# your own real project data, without you having to remember the dance of
# building with a guest-local CARGO_TARGET_DIR, starting a real systemd
# --user session, and checking every uninstall touchpoint by hand.
#
# Requires a running Lima VM (`limactl start default` if you don't have one
# yet) whose config mounts your home directory at the same absolute path
# inside the guest — that is Lima's out-of-the-box default, not something
# this script sets up itself.
#
# Usage:
#   petridish-cli/scripts/lima-dev.sh <command>
#
#   build       Build petridish/swab/petri inside the VM (idempotent).
#   install     Build, then a real `petridish install` against systemd
#               --user. Leaves the timer running — go run `petri` yourself
#               afterward (see `shell` below) to see the real dashboard.
#   uninstall   `petridish uninstall`, then verify it left no trace.
#   verify      Re-run just the post-uninstall touchpoint checks.
#   smoke       install -> doctor -> one real `swab scan` -> `petri
#               --version` -> uninstall -> verify. The full round trip, no
#               interactive petri session required. ("roundtrip" also works.)
#   shell       Interactive shell in the VM with PATH already pointed at the
#               built binaries, for running the real `petri` dashboard.
#
# LIMA_INSTANCE overrides the VM name (default: "default").
set -euo pipefail

INSTANCE="${LIMA_INSTANCE:-default}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GUEST_SCRIPT="$REPO_ROOT/petridish-cli/scripts/lima-guest.sh"

cmd="${1:-}"
if [[ -z "$cmd" ]]; then
  echo "usage: $0 <build|install|uninstall|verify|smoke|shell>" >&2
  exit 2
fi

if ! limactl list --format '{{.Name}}' 2>/dev/null | grep -qx "$INSTANCE"; then
  echo "no Lima instance named '$INSTANCE' — run 'limactl start $INSTANCE' first" >&2
  exit 1
fi

if [[ "$cmd" == "shell" ]]; then
  echo "Dropping into '$INSTANCE' with PATH set for the last build..." >&2
  exec limactl shell "$INSTANCE" -- bash -lc 'export PATH="$HOME/pd-target/debug:$PATH"; exec bash -i'
fi

# These mirror swab's own defaults (~/repos, ~/learning) but anchored at the
# HOST home directory's absolute path, since that is what Lima's default
# mount preserves 1:1 inside the guest. Deliberately never hardcode a
# specific user's home here — the same "never hardcode the path" rule
# ARCHITECTURE.md's D1 holds the installer itself to.
#
# `limactl shell` execs the remote command over SSH — it does NOT forward
# this script's own exported env vars into the guest. `env KEY=VAL ... cmd`
# is the standard way to set them on the *remote* side instead: `env` itself
# is what runs remotely, as the first word of the forwarded argv.
PD_REPO_ROOT="$REPO_ROOT"
PD_DEFAULT_ROOTS="$HOME/repos,$HOME/learning"

exec limactl shell "$INSTANCE" -- \
  env "PD_REPO_ROOT=$PD_REPO_ROOT" "PD_DEFAULT_ROOTS=$PD_DEFAULT_ROOTS" \
  bash "$GUEST_SCRIPT" "$cmd"
