"""Installer for the petridish launchd job and Claude Code hook wiring.

This is the one module in the package that mutates state outside its own
data directory: ``~/.claude/settings.json`` (shared with other hook
consumers — pixtuoid, statusbar, notchbar) and ``~/Library/LaunchAgents``.
Every edit to ``settings.json`` is *structural*: it only ever adds or removes
dict entries that carry :data:`petridish.schema.HOOK_MARKER` somewhere in
their subtree. Nothing here ever reserialises or touches an entry it did not
add itself — see ARCHITECTURE.md §8.3 D4.

A backup of the pre-install ``settings.json`` is written once, but it is a
safety artifact for the human, never read back automatically. Restoring from
it on uninstall would silently discard any unrelated edits made to the file
between install and uninstall (other hook consumers reinstalling, `/model`
writes, manual edits) — see ARCHITECTURE.md §8.3 D4's "removes only marked entries".
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Callable

from petridish.schema import HOOK_MARKER

PLIST_LABEL = "com.petridish.daemon"

#: Default filename for the menubar xbar/SwiftBar plugin. Matches the name of
#: the template in ``resources/petridish_menubar.30s.py``.
DEFAULT_MENUBAR_PLUGIN_FILENAME = "petridish_menubar.30s.py"

def default_menubar_plugins_dir(home: Path) -> Path:
    """xbar's real default plugin directory on macOS, anchored on `home`.

    SwiftBar's directory is user-configured and cannot be guessed, so xbar's
    default is the right fallback. Takes `home` explicitly (never reads
    `Path.home()` itself) so it stays testable like every other path in this
    module.
    """
    return home / "Library" / "Application Support" / "xbar" / "plugins"

#: Events the hook is registered on: PreToolUse for in-session liveness,
#: Stop for turn-end. Both accept matcher-less hook groups in the real
#: settings.json (verified against this machine's file).
HOOK_EVENTS: tuple[str, ...] = ("PreToolUse", "Stop")

DEFAULT_CONFIG_TOML = """\
# petridish config — every field below is optional; these are the defaults.
# Uncomment and edit to override.
#
# roots = ["~/repos", "~/learning"]
# extra_paths = []
# author_patterns = ["Jan.*Krag"]
# author_since = "3 years"
# max_depth = 4
"""

Runner = Callable[[list[str]], "subprocess.CompletedProcess[str]"]


class InstallError(RuntimeError):
    """A condition that should abort install/uninstall with a clear message."""


# ---------------------------------------------------------------------------
# Preconditions
# ---------------------------------------------------------------------------

def check_platform() -> None:
    if sys.platform != "darwin":
        raise InstallError(
            f"petridish only supports macOS (launchd); detected {sys.platform!r}."
        )


def resolve_binary(name: str) -> str:
    """Resolve `name` to an absolute path via PATH. Never hardcode (D1)."""
    path = shutil.which(name)
    if not path:
        raise InstallError(
            f"{name!r} not found on PATH. Install it first: "
            "uv tool install --editable . (see ARCHITECTURE.md §8.1)."
        )
    return os.path.abspath(path)


# ---------------------------------------------------------------------------
# User-data directory (D6 — survives uninstall/reinstall untouched)
# ---------------------------------------------------------------------------

def ensure_data_dir(data_dir: Path) -> None:
    data_dir.mkdir(parents=True, exist_ok=True)


def write_default_config(data_dir: Path) -> bool:
    """Write config.toml only if absent. Returns True iff it wrote a file."""
    config_path = data_dir / "config.toml"
    if config_path.exists():
        return False
    config_path.write_text(DEFAULT_CONFIG_TOML, encoding="utf-8")
    return True


# ---------------------------------------------------------------------------
# settings.json — structural hook wiring
# ---------------------------------------------------------------------------

def _contains_marker(obj: object, marker: str) -> bool:
    if isinstance(obj, str):
        return marker in obj
    if isinstance(obj, dict):
        return any(_contains_marker(v, marker) for v in obj.values())
    if isinstance(obj, list):
        return any(_contains_marker(v, marker) for v in obj)
    return False


def has_marker(settings: dict, marker: str = HOOK_MARKER) -> bool:
    return _contains_marker(settings, marker)


def add_hook_entries(
    settings: dict, hook_abspath: str, marker: str = HOOK_MARKER
) -> dict:
    """Return a settings dict with one hook-group appended per HOOK_EVENTS.

    Idempotent: if `marker` is already present anywhere, returns `settings`
    unchanged (same object) so callers can tell nothing needs writing.
    """
    if has_marker(settings, marker):
        return settings

    updated = dict(settings)
    hooks = dict(updated.get("hooks", {}))
    command = f"{hook_abspath} {marker}"
    for event in HOOK_EVENTS:
        group = {"hooks": [{"type": "command", "command": command}]}
        hooks[event] = [*hooks.get(event, []), group]
    updated["hooks"] = hooks
    return updated


def _drop_marked(obj: object, marker: str) -> object:
    """Recursively drop list elements whose subtree contains `marker`."""
    if isinstance(obj, list):
        return [
            _drop_marked(item, marker)
            for item in obj
            if not (isinstance(item, dict) and _contains_marker(item, marker))
        ]
    if isinstance(obj, dict):
        return {k: _drop_marked(v, marker) for k, v in obj.items()}
    return obj


def remove_marker_entries(settings: dict, marker: str = HOOK_MARKER) -> dict:
    """Structurally remove every hook-group tagged with `marker`.

    Only drops dict elements *of a list* whose subtree contains the marker —
    exactly the shape :func:`add_hook_entries` adds. Sibling entries (other
    hook consumers) and every other key are passed through unchanged.
    """
    return _drop_marked(settings, marker)


def serialize_settings(data: dict) -> str:
    return json.dumps(data, indent=2) + "\n"


def load_settings(settings_path: Path) -> dict:
    if not settings_path.exists():
        return {}
    text = settings_path.read_text(encoding="utf-8")
    if not text.strip():
        return {}
    return json.loads(text)


def write_settings_atomic(settings_path: Path, data: dict) -> None:
    settings_path.parent.mkdir(parents=True, exist_ok=True)
    tmp = settings_path.with_suffix(settings_path.suffix + ".tmp")
    tmp.write_text(serialize_settings(data), encoding="utf-8")
    os.replace(tmp, settings_path)


def backup_settings(settings_path: Path, backup_path: Path) -> None:
    """Copy the current settings.json to `backup_path`, only if absent.

    Only ever taken once — a second install (or a re-run after the hook is
    already wired) must not overwrite a clean pre-install backup with a
    dirty one that already contains our own entries.
    """
    if backup_path.exists():
        return
    backup_path.parent.mkdir(parents=True, exist_ok=True)
    if settings_path.exists():
        shutil.copy2(settings_path, backup_path)
    else:
        backup_path.write_text("", encoding="utf-8")


# ---------------------------------------------------------------------------
# launchd plist
# ---------------------------------------------------------------------------

def _read_plist_template(label: str, resources_dir: Path | None) -> str:
    """Read the plist template text.

    `resources_dir`, when given, is a plain filesystem override (used by
    tests). Otherwise this reads via :mod:`importlib.resources`, anchored on
    the *installed* ``petridish`` package — this is what makes it work both
    for an editable install (source tree intact) and a real wheel install
    (``src/petridish/resources/`` is shipped as package data; see
    ``[tool.setuptools.package-data]`` in pyproject.toml).
    """
    filename = f"{label}.plist"
    if resources_dir is not None:
        template_path = resources_dir / filename
        if not template_path.is_file():
            raise InstallError(f"plist template not found at {template_path}")
        return template_path.read_text(encoding="utf-8")

    import importlib.resources

    traversable = importlib.resources.files("petridish").joinpath("resources", filename)
    if not traversable.is_file():
        raise InstallError(
            f"plist template {filename!r} not found in the installed "
            "petridish package's resources/ (expected it to ship as package "
            "data — see [tool.setuptools.package-data] in pyproject.toml)."
        )
    return traversable.read_text(encoding="utf-8")


def render_plist(
    *,
    swab_abspath: str,
    log_path: str,
    label: str = PLIST_LABEL,
    resources_dir: Path | None = None,
) -> str:
    from xml.sax.saxutils import escape

    text = _read_plist_template(label, resources_dir)
    return (
        text.replace("__LABEL__", escape(label))
        .replace("__SWAB_PATH__", escape(swab_abspath))
        .replace("__LOG_PATH__", escape(log_path))
    )


def render_menubar_plugin(*, python_executable: str, resources_dir: Path | None = None) -> str:
    """Render the menubar xbar/SwiftBar plugin script with the correct shebang.

    Reads ``resources/petridish_menubar.30s.py`` and substitutes the
    ``__PYTHON_SHEBANG__`` placeholder with ``python_executable``. No XML
    escaping is needed — this is a plain script, not a plist.

    Raises :class:`InstallError` if the template file is missing from the
    given ``resources_dir`` or the installed package's resources/.
    """
    template_filename = "petridish_menubar.30s.py"
    text = _read_menubar_template(template_filename, resources_dir)
    return text.replace("__PYTHON_SHEBANG__", python_executable)


def _read_menubar_template(filename: str, resources_dir: Path | None) -> str:
    """Read the menubar plugin template text.

    Mirrors the layout of :func:`_read_plist_template`: ``resources_dir`` is a
    plain filesystem override used by tests; otherwise the file is read via
    :mod:`importlib.resources`, anchored on the installed ``petridish``
    package. Raises :class:`InstallError` if the template is missing.
    """
    if resources_dir is not None:
        template_path = resources_dir / filename
        if not template_path.is_file():
            raise InstallError(f"template not found at {template_path}")
        return template_path.read_text(encoding="utf-8")

    import importlib.resources

    traversable = importlib.resources.files("petridish").joinpath("resources", filename)
    if not traversable.is_file():
        raise InstallError(
            f"template {filename!r} not found in the installed "
            "petridish package's resources/ (expected it to ship as package "
            "data — see [tool.setuptools.package-data] in pyproject.toml)."
        )
    return traversable.read_text(encoding="utf-8")


def write_plist(plist_path: Path, content: str) -> None:
    plist_path.parent.mkdir(parents=True, exist_ok=True)
    plist_path.write_text(content, encoding="utf-8")


def write_menubar_plugin(plugin_path: Path, content: str) -> None:
    """Write a menubar plugin script to ``plugin_path`` and make it executable.

    Unlike :func:`write_plist`, a plist is a data file that launchd reads
    verbatim — it must never be +x. A menubar plugin script (xbar/SwiftBar)
    is executed by an external interpreter and MUST be +x to run. Mode set
    to ``0o755`` (rwxr-xr-x).
    """
    plugin_path.parent.mkdir(parents=True, exist_ok=True)
    plugin_path.write_text(content, encoding="utf-8")
    plugin_path.chmod(0o755)


def _default_runner(args: list[str]) -> "subprocess.CompletedProcess[str]":
    return subprocess.run(
        args, capture_output=True, text=True, timeout=10, check=False
    )


def load_job(
    plist_path: Path,
    uid: int,
    runner: Runner = _default_runner,
    label: str = PLIST_LABEL,
) -> None:
    """Register the plist with launchd, replacing any stale registration.

    EALREADY means launchd already has a job under this label loaded — but
    from *its* in-memory definition, which may point at a binary that has
    since moved or been removed (e.g. after a `swab` rebuild/reinstall).
    Just returning here — the old behavior — left that stale definition
    running forever; every subsequent reinstall would rewrite the plist file
    on disk but launchd would keep executing the old program path. Boot the
    stale job out first so the fresh plist actually takes effect.
    """
    result = runner(["launchctl", "bootstrap", f"gui/{uid}", str(plist_path)])
    if result.returncode == 0:
        return
    if result.returncode == 5:  # EALREADY
        runner(["launchctl", "bootout", f"gui/{uid}/{label}"])
        result = runner(["launchctl", "bootstrap", f"gui/{uid}", str(plist_path)])
        if result.returncode == 0:
            return
    # Fallback for systems/domains where bootstrap semantics differ.
    fallback = runner(["launchctl", "load", "-w", str(plist_path)])
    if fallback.returncode != 0:
        raise InstallError(
            "launchctl failed to load the job:\n"
            f"  bootstrap: {result.stderr.strip()}\n"
            f"  load -w:   {fallback.stderr.strip()}"
        )


def unload_job(label: str, uid: int, runner: Runner = _default_runner) -> None:
    """Unregister the job. Tolerates "not loaded"."""
    result = runner(["launchctl", "bootout", f"gui/{uid}/{label}"])
    if result.returncode in (0, 3):  # 3 = ESRCH, not loaded
        return
    fallback = runner(["launchctl", "remove", label])
    if fallback.returncode != 0:
        # Not fatal — the plist file is about to be removed either way, and
        # a stale launchd registration for a missing plist is harmless noise
        # macOS clears on next login. Surface it; don't abort uninstall over it.
        print(
            f"warning: launchctl could not unload {label} cleanly "
            f"({result.stderr.strip() or fallback.stderr.strip()})",
            file=sys.stderr,
        )


# ---------------------------------------------------------------------------
# Orchestration
# ---------------------------------------------------------------------------

def install(
    *,
    home: Path,
    claude_dir: Path,
    launch_agents_dir: Path,
    uid: int,
    runner: Runner = _default_runner,
    menubar_plugins_dir: Path | None = None,
) -> int:
    check_platform()

    hook_abspath = resolve_binary("swab-hook")
    swab_abspath = resolve_binary("swab")

    data_dir = home / ".petridish"
    ensure_data_dir(data_dir)
    wrote_config = write_default_config(data_dir)
    print(f"{'wrote' if wrote_config else 'kept existing'} config: {data_dir / 'config.toml'}")

    settings_path = claude_dir / "settings.json"
    backup_path = data_dir / "settings.json.backup"
    backup_settings(settings_path, backup_path)

    settings = load_settings(settings_path)
    updated = add_hook_entries(settings, hook_abspath)
    if updated is settings:
        print("hook already installed in settings.json (marker present); left untouched")
    else:
        write_settings_atomic(settings_path, updated)
        print(f"added hook entries to {settings_path}")
    print(f"pre-install backup kept at {backup_path}")

    plist_path = launch_agents_dir / f"{PLIST_LABEL}.plist"
    content = render_plist(
        swab_abspath=swab_abspath, log_path=str(data_dir / "daemon.log")
    )
    write_plist(plist_path, content)
    load_job(plist_path, uid, runner)
    print(f"launchd job loaded: {plist_path}")

    # Menubar plugin: always re-render/write on every install call.
    if menubar_plugins_dir is not None:
        plugin_filename = DEFAULT_MENUBAR_PLUGIN_FILENAME
        plugin_path = menubar_plugins_dir / plugin_filename
        content = render_menubar_plugin(python_executable=sys.executable)
        write_menubar_plugin(plugin_path, content)
        print(f"menubar plugin installed: {plugin_path}")

    return 0


def uninstall(
    *,
    home: Path,
    claude_dir: Path,
    launch_agents_dir: Path,
    uid: int,
    runner: Runner = _default_runner,
    menubar_plugins_dir: Path | None = None,
) -> int:
    plist_path = launch_agents_dir / f"{PLIST_LABEL}.plist"
    unload_job(PLIST_LABEL, uid, runner)
    if plist_path.exists():
        plist_path.unlink()
        print(f"removed {plist_path}")

    settings_path = claude_dir / "settings.json"
    settings = load_settings(settings_path)
    if has_marker(settings):
        cleaned = remove_marker_entries(settings)
        write_settings_atomic(settings_path, cleaned)
        print(f"removed hook entries from {settings_path}")
    else:
        print("no hook entries found in settings.json; nothing to remove")

    # Menubar plugin: only remove if installed (file exists) at the given dir.
    if menubar_plugins_dir is not None:
        plugin_filename = DEFAULT_MENUBAR_PLUGIN_FILENAME
        plugin_path = menubar_plugins_dir / plugin_filename
        if plugin_path.exists():
            plugin_path.unlink()
            print(f"removed {plugin_path}")
        else:
            print("nothing to remove: menubar plugin not found at given directory")

    data_dir = home / ".petridish"
    backup_path = data_dir / "settings.json.backup"
    if backup_path.exists():
        print(f"pre-install backup left in place at {backup_path} (not restored automatically)")
    print(f"user data left in place at {data_dir} (config.toml, projects.json, events.ndjson)")

    return 0


def main(argv: list[str] | None = None) -> int:
    import argparse

    parser = argparse.ArgumentParser(prog="petridish-installer")
    parser.add_argument("--uninstall", action="store_true")
    parser.add_argument(
        "--menubar-plugins-dir",
        type=Path,
        default=None,
        help="Override the target directory for the menubar xbar/SwiftBar "
             "plugin. When omitted, defaults to "
             "~/Library/Application Support/xbar/plugins.",
    )
    # Boolean opt-out so users without xbar/SwiftBar can skip the plugin
    # step without having to figure out the default directory layout.
    parser.add_argument(
        "--no-menubar-plugin",
        action="store_true",
        default=False,
        dest="skip_menubar_plugin",
        help="Skip installing the menubar plugin. Useful when xbar/SwiftBar "
             "is not installed.",
    )
    args = parser.parse_args(argv)

    home = Path.home()
    claude_dir = home / ".claude"
    launch_agents_dir = home / "Library" / "LaunchAgents"
    uid = os.getuid()

    # Menubar plugin directory resolution:
    #   --skip-menubar-plugin → None (skip entirely)
    #   --menubar-plugins-dir <p> → <p>
    #   otherwise → default (xbar's real default plugins directory on macOS).
    if args.skip_menubar_plugin:
        menubar_plugins_dir: Path | None = None
    elif args.menubar_plugins_dir is not None:
        menubar_plugins_dir = args.menubar_plugins_dir
    else:
        menubar_plugins_dir = default_menubar_plugins_dir(home)

    try:
        if args.uninstall:
            return uninstall(
                home=home, claude_dir=claude_dir,
                launch_agents_dir=launch_agents_dir, uid=uid,
                menubar_plugins_dir=menubar_plugins_dir,
            )
        return install(
            home=home, claude_dir=claude_dir,
            launch_agents_dir=launch_agents_dir, uid=uid,
            menubar_plugins_dir=menubar_plugins_dir,
        )
    except InstallError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
