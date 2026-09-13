#!/usr/bin/env bash
#
# petridish-cli/scripts/lima-guest.sh — runs INSIDE the Lima VM.
#
# Invoked by lima-dev.sh over `limactl shell`, via the virtiofs path that
# mirrors this repo's host path 1:1 inside the guest — never call this
# directly from macOS. It exercises petridish's real systemd --user surface:
# `cargo test`'s coverage all goes through the RecordingSystemctl seam (real
# for argv/ordering/error-mapping, but never the real `systemctl` binary or a
# real unit search path). This is that real thing, on demand.
#
# PD_REPO_ROOT and PD_DEFAULT_ROOTS are set by lima-dev.sh, not read from the
# guest's own environment — deriving them here would mean guessing at the
# host's home directory from inside the VM, which is exactly the kind of
# hardcoded-path mistake ARCHITECTURE.md's D1 warns the installer itself away
# from.
set -euo pipefail

CMD="${1:-}"

# NOT under the repo mount: that mount is read-only (Lima's default), so
# `cargo build`'s target/ directory has to live in the guest's own writable
# home instead.
GUEST_TARGET_DIR="$HOME/pd-target"
BIN_DIR="$GUEST_TARGET_DIR/debug"

: "${PD_REPO_ROOT:?set by lima-dev.sh}"
: "${PD_DEFAULT_ROOTS:?set by lima-dev.sh}"

pass=0
fail=0

run_check() {
  local desc="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    echo "  ok:   $desc"
    pass=$((pass + 1))
  else
    echo "  FAIL: $desc"
    fail=$((fail + 1))
  fi
}

ensure_toolchain() {
  if ! command -v cc >/dev/null 2>&1; then
    echo "installing build-essential..."
    sudo apt-get update -qq
    sudo apt-get install -y -qq build-essential
  fi
  if [[ ! -e "$HOME/.cargo/env" ]]; then
    echo "installing rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y -q
  fi
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env"
}

do_build() {
  ensure_toolchain
  export CARGO_TARGET_DIR="$GUEST_TARGET_DIR"
  (cd "$PD_REPO_ROOT" && cargo build --locked -p petri-dish -p swab -p petri --bins)
}

require_built() {
  if [[ ! -x "$BIN_DIR/petridish" ]]; then
    echo "no build found at $BIN_DIR — run 'build' or 'install' first" >&2
    exit 1
  fi
}

# Only fills in the placeholder line install.rs's DEFAULT_CONFIG_TOML ships
# commented out. Never touches it once you've uncommented it yourself — same
# "write once, never overwrite" rule install.rs itself applies to config.toml
# as a whole (see `an_existing_config_toml_is_never_overwritten`).
set_default_roots_if_untouched() {
  local cfg="$HOME/.petridish/config.toml"
  [[ -f "$cfg" ]] || return 0
  grep -qE '^# roots = ' "$cfg" || return 0
  python3 - "$cfg" "$PD_DEFAULT_ROOTS" <<'PYEOF'
import sys
cfg_path, roots_csv = sys.argv[1], sys.argv[2]
roots = roots_csv.split(",")
new_line = "roots = [" + ", ".join(repr(r) for r in roots) + "]\n"
with open(cfg_path) as f:
    lines = f.readlines()
with open(cfg_path, "w") as f:
    for line in lines:
        f.write(new_line if line.startswith("# roots = ") else line)
PYEOF
  echo "config.toml: filled in default roots -> $PD_DEFAULT_ROOTS"
}

do_install() {
  do_build
  export PATH="$BIN_DIR:$PATH"
  petridish install --no-menubar-plugin
  set_default_roots_if_untouched
  echo
  petridish doctor
}

timer_removed() { ! systemctl --user is-enabled petridish-scan.timer; }
timer_inactive() { ! systemctl --user is-active petridish-scan.timer; }
timer_file_gone() { [[ ! -f "$HOME/.config/systemd/user/petridish-scan.timer" ]]; }
service_file_gone() { [[ ! -f "$HOME/.config/systemd/user/petridish-scan.service" ]]; }
hook_removed() { ! grep -q '# petridish' "$HOME/.claude/settings.json"; }
config_survived() { [[ -f "$HOME/.petridish/config.toml" ]]; }

do_verify() {
  echo "verifying uninstall touchpoints..."
  run_check "timer unit removed from systemd" timer_removed
  run_check "timer is not active" timer_inactive
  run_check "timer unit file absent" timer_file_gone
  run_check "service unit file absent" service_file_gone
  if [[ -f "$HOME/.claude/settings.json" ]]; then
    run_check "hook marker removed from settings.json" hook_removed
  fi
  if [[ -f "$HOME/.petridish/config.toml" ]]; then
    run_check "user data survived uninstall (D6)" config_survived
  fi
  echo
  echo "$pass passed, $fail failed"
  [[ $fail -eq 0 ]]
}

do_smoke() {
  do_install

  echo
  echo "checking doctor reports no failures post-install..."
  if petridish doctor | grep -q '^fail:'; then
    echo "doctor reported a failing check right after install — aborting" >&2
    exit 1
  fi

  echo "running a real scan..."
  swab scan

  python3 - "$HOME/.petridish/projects.json" <<'PYEOF'
import json
import sys

with open(sys.argv[1]) as f:
    data = json.load(f)
projects = data.get("projects", [])
assert isinstance(projects, list) and len(projects) > 0, (
    f"expected at least one scanned project, got {projects!r}"
)
print(f"scan found {len(projects)} project(s)")
PYEOF

  petri --version

  echo
  echo "uninstalling..."
  petridish uninstall --no-menubar-plugin
  do_verify
}

export PATH="$BIN_DIR:$PATH"

case "$CMD" in
  build)
    do_build
    ;;
  install)
    do_install
    ;;
  uninstall)
    require_built
    petridish uninstall --no-menubar-plugin
    do_verify
    ;;
  verify)
    do_verify
    ;;
  smoke | roundtrip)
    do_smoke
    ;;
  *)
    echo "usage: lima-guest.sh <build|install|uninstall|verify|smoke>" >&2
    exit 2
    ;;
esac
