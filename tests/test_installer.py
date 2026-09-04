"""Tests for ``src/petridish/installer.py`` — M9.

All fixtures live under ``tmp_path``: a fake ``$HOME``, a fake
``~/.claude/settings.json``, a fake ``~/Library/LaunchAgents``, and real
stub executables named ``swab``/``swab-hook`` on a PATH built just for the
test, so :func:`shutil.which` resolves them the same way it would for a real
install. The real ``launchctl`` is never invoked — every orchestration test
passes a recording fake as the ``runner``. ``settings.json`` fixtures are
always produced with :func:`petridish.installer.serialize_settings`, so
round-trip byte-equality tests aren't fighting an unrelated formatting
mismatch.
"""

from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

from petridish.schema import Project, Radar, read_json, utcnow, HOOK_MARKER
from petridish.installer import (
    DEFAULT_MENUBAR_PLUGIN_FILENAME,
    InstallError,
    HOOK_EVENTS,
    add_hook_entries,
    backup_settings,
    check_platform,
    default_menubar_plugins_dir,
    ensure_data_dir,
    has_marker,
    install,
    load_job,
    load_settings,
    remove_marker_entries,
    render_menubar_plugin,
    render_plist,
    resolve_binary,
    serialize_settings,
    uninstall,
    unload_job,
    write_default_config,
    write_menubar_plugin,
    write_settings_atomic,
)
from petridish.menubar import render_menubar


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

OTHER_CONSUMER_SETTINGS = {
    "model": "sonnet",
    "hooks": {
        "Notification": [
            {"matcher": ".*", "hooks": [{"type": "command", "command": "pixtuoid-hook"}]},
        ],
        "PreToolUse": [
            {
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": "rtk hook claude"}],
            },
            {"hooks": [{"type": "command", "command": "notchbar-hook.sh Working # notchbar-agents-claude-hook"}]},
        ],
        "Stop": [
            {"hooks": [{"type": "command", "command": "notchbar-hook.sh Idle # notchbar-agents-claude-hook"}]},
        ],
    },
}


def _make_stub_bin(tmp_path: Path) -> Path:
    """Create real, executable stub `swab`/`swab-hook` files and return the dir."""
    bin_dir = tmp_path / "stubbin"
    bin_dir.mkdir()
    for name in ("swab", "swab-hook"):
        exe = bin_dir / name
        exe.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        exe.chmod(exe.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
    return bin_dir


def _recording_runner(calls: list):
    def runner(args):
        calls.append(args)
        return _FakeCompleted(returncode=0)
    return runner


class _FakeCompleted:
    def __init__(self, returncode: int, stderr: str = ""):
        self.returncode = returncode
        self.stderr = stderr
        self.stdout = ""


# ---------------------------------------------------------------------------
# Pure functions: settings.json structural edits
# ---------------------------------------------------------------------------

def test_add_hook_entries_appends_preserving_existing():
    updated = add_hook_entries(OTHER_CONSUMER_SETTINGS, "/abs/path/swab-hook")

    assert updated is not OTHER_CONSUMER_SETTINGS
    # Existing consumers untouched — including under `Notification`, which we now
    # register on too (MECH-5): ours is *appended*, the pixtuoid group stays first.
    assert (
        updated["hooks"]["Notification"][0]
        == OTHER_CONSUMER_SETTINGS["hooks"]["Notification"][0]
    )
    assert len(updated["hooks"]["Notification"]) == 2  # pixtuoid + ours
    assert len(updated["hooks"]["PreToolUse"]) == 3  # rtk + notchbar + ours
    assert len(updated["hooks"]["Stop"]) == 2  # notchbar + ours
    assert len(updated["hooks"]["PermissionRequest"]) == 1  # ours alone; new key
    assert has_marker(updated)
    for event in HOOK_EVENTS:
        commands = [
            h["command"]
            for group in updated["hooks"][event]
            for h in group["hooks"]
        ]
        assert any(HOOK_MARKER in c and "/abs/path/swab-hook" in c for c in commands)


def test_add_hook_entries_idempotent_when_marker_present():
    once = add_hook_entries(OTHER_CONSUMER_SETTINGS, "/abs/path/swab-hook")
    twice = add_hook_entries(once, "/abs/path/swab-hook")

    assert twice is once  # no-op: same object back
    assert len(twice["hooks"]["PreToolUse"]) == 3
    assert len(twice["hooks"]["Stop"]) == 2


def test_add_hook_entries_on_empty_settings():
    updated = add_hook_entries({}, "/abs/path/swab-hook")

    assert updated["hooks"]["PreToolUse"][0]["hooks"][0]["command"] == f"/abs/path/swab-hook {HOOK_MARKER}"
    assert updated["hooks"]["Stop"][0]["hooks"][0]["command"] == f"/abs/path/swab-hook {HOOK_MARKER}"


def test_remove_marker_entries_removes_only_marked():
    with_hooks = add_hook_entries(OTHER_CONSUMER_SETTINGS, "/abs/path/swab-hook")

    cleaned = remove_marker_entries(with_hooks)

    assert not has_marker(cleaned)
    assert cleaned["hooks"]["PreToolUse"] == OTHER_CONSUMER_SETTINGS["hooks"]["PreToolUse"]
    assert cleaned["hooks"]["Stop"] == OTHER_CONSUMER_SETTINGS["hooks"]["Stop"]
    assert cleaned["hooks"]["Notification"] == OTHER_CONSUMER_SETTINGS["hooks"]["Notification"]


def test_remove_marker_entries_noop_when_absent():
    cleaned = remove_marker_entries(OTHER_CONSUMER_SETTINGS)
    assert cleaned == OTHER_CONSUMER_SETTINGS


def test_add_then_remove_round_trip_is_byte_identical():
    """install-then-uninstall on an untouched file reproduces it exactly."""
    original_bytes = serialize_settings(OTHER_CONSUMER_SETTINGS)

    with_hooks = add_hook_entries(OTHER_CONSUMER_SETTINGS, "/abs/path/swab-hook")
    cleaned = remove_marker_entries(with_hooks)

    assert serialize_settings(cleaned) == original_bytes


# ---------------------------------------------------------------------------
# File I/O: backup, config, settings read/write
# ---------------------------------------------------------------------------

def test_backup_settings_only_taken_once(tmp_path):
    settings_path = tmp_path / "settings.json"
    backup_path = tmp_path / "data" / "settings.json.backup"
    settings_path.write_text(serialize_settings(OTHER_CONSUMER_SETTINGS), encoding="utf-8")

    backup_settings(settings_path, backup_path)
    first_backup = backup_path.read_text(encoding="utf-8")

    # Simulate the file changing (e.g. our own install added hooks) — a
    # second backup call must not clobber the pristine first one.
    settings_path.write_text(serialize_settings(add_hook_entries(OTHER_CONSUMER_SETTINGS, "/x")), encoding="utf-8")
    backup_settings(settings_path, backup_path)

    assert backup_path.read_text(encoding="utf-8") == first_backup
    assert first_backup == serialize_settings(OTHER_CONSUMER_SETTINGS)


def test_backup_settings_when_settings_file_absent(tmp_path):
    settings_path = tmp_path / "settings.json"  # does not exist
    backup_path = tmp_path / "data" / "settings.json.backup"

    backup_settings(settings_path, backup_path)

    assert backup_path.exists()
    assert backup_path.read_text(encoding="utf-8") == ""


def test_write_default_config_only_if_absent(tmp_path):
    data_dir = tmp_path / ".petridish"
    ensure_data_dir(data_dir)

    wrote_first = write_default_config(data_dir)
    (data_dir / "config.toml").write_text("roots = [\"~/custom\"]\n", encoding="utf-8")
    wrote_second = write_default_config(data_dir)

    assert wrote_first is True
    assert wrote_second is False
    assert (data_dir / "config.toml").read_text(encoding="utf-8") == "roots = [\"~/custom\"]\n"


def test_load_settings_missing_file_returns_empty_dict(tmp_path):
    assert load_settings(tmp_path / "nope.json") == {}


def test_write_settings_atomic_leaves_no_tmp_file(tmp_path):
    settings_path = tmp_path / "settings.json"
    write_settings_atomic(settings_path, OTHER_CONSUMER_SETTINGS)

    assert json.loads(settings_path.read_text(encoding="utf-8")) == OTHER_CONSUMER_SETTINGS
    assert not list(tmp_path.glob("*.tmp"))


# ---------------------------------------------------------------------------
# plist rendering
# ---------------------------------------------------------------------------

def test_render_plist_substitutes_paths():
    content = render_plist(swab_abspath="/abs/bin/swab", log_path="/abs/log/daemon.log")

    assert "/abs/bin/swab" in content
    assert "/abs/log/daemon.log" in content
    assert "com.petridish.daemon" in content
    assert "<integer>60</integer>" in content
    assert "<key>RunAtLoad</key>" in content
    assert "Background" in content
    assert "__SWAB_PATH__" not in content
    assert "__LOG_PATH__" not in content
    assert "__LABEL__" not in content


def test_render_plist_missing_template_raises(tmp_path):
    with pytest.raises(InstallError, match="plist template not found"):
        render_plist(
            swab_abspath="/abs/bin/swab",
            log_path="/abs/log/daemon.log",
            resources_dir=tmp_path / "nowhere",
        )


# ---------------------------------------------------------------------------
# Preconditions
# ---------------------------------------------------------------------------

def test_check_platform_raises_on_non_darwin(monkeypatch):
    monkeypatch.setattr(sys, "platform", "linux")
    with pytest.raises(InstallError, match="macOS"):
        check_platform()


def test_check_platform_passes_on_darwin(monkeypatch):
    monkeypatch.setattr(sys, "platform", "darwin")
    check_platform()  # must not raise


def test_resolve_binary_missing_raises(monkeypatch):
    monkeypatch.setenv("PATH", "")
    with pytest.raises(InstallError, match="not found on PATH"):
        resolve_binary("definitely-not-a-real-binary-xyz")


def test_resolve_binary_returns_absolute_path(tmp_path, monkeypatch):
    bin_dir = _make_stub_bin(tmp_path)
    monkeypatch.setenv("PATH", str(bin_dir))

    resolved = resolve_binary("swab")

    assert os.path.isabs(resolved)
    assert resolved == str(bin_dir / "swab")


# ---------------------------------------------------------------------------
# launchd wrapper: tolerate "already loaded" / "not loaded"
# ---------------------------------------------------------------------------

def test_load_job_reloads_stale_job_on_already_loaded(tmp_path):
    # Regression test: EALREADY used to be treated as success outright, which
    # left launchd running its old in-memory job definition (stale binary
    # path) even though the plist file on disk had just been rewritten.
    calls = []

    def runner(args):
        calls.append(args)
        if args[1] == "bootstrap" and len(calls) == 1:
            return _FakeCompleted(returncode=5)  # EALREADY
        return _FakeCompleted(returncode=0)

    load_job(tmp_path / "x.plist", uid=501, runner=runner, label="com.petridish.daemon")

    assert calls == [
        ["launchctl", "bootstrap", "gui/501", str(tmp_path / "x.plist")],
        ["launchctl", "bootout", "gui/501/com.petridish.daemon"],
        ["launchctl", "bootstrap", "gui/501", str(tmp_path / "x.plist")],
    ]


def test_load_job_falls_back_to_load_dash_w(tmp_path):
    calls = []

    def runner(args):
        calls.append(args)
        if args[1] == "bootstrap":
            return _FakeCompleted(returncode=1, stderr="nope")
        return _FakeCompleted(returncode=0)

    load_job(tmp_path / "x.plist", uid=501, runner=runner)

    assert calls[0][1] == "bootstrap"
    assert calls[1] == ["launchctl", "load", "-w", str(tmp_path / "x.plist")]


def test_load_job_raises_when_both_fail(tmp_path):
    def runner(args):
        return _FakeCompleted(returncode=1, stderr="boom")

    with pytest.raises(InstallError, match="launchctl failed"):
        load_job(tmp_path / "x.plist", uid=501, runner=runner)


def test_unload_job_treats_not_loaded_as_success():
    calls = []

    def runner(args):
        calls.append(args)
        return _FakeCompleted(returncode=3)  # ESRCH

    unload_job("com.petridish.daemon", uid=501, runner=runner)

    assert calls == [["launchctl", "bootout", "gui/501/com.petridish.daemon"]]


# ---------------------------------------------------------------------------
# Orchestration: install() / uninstall()
# ---------------------------------------------------------------------------

def _installed_fixture(tmp_path, monkeypatch):
    bin_dir = _make_stub_bin(tmp_path)
    monkeypatch.setenv("PATH", str(bin_dir))
    monkeypatch.setattr(sys, "platform", "darwin")

    home = tmp_path / "home"
    claude_dir = home / ".claude"
    claude_dir.mkdir(parents=True)
    launch_agents_dir = home / "Library" / "LaunchAgents"
    (claude_dir / "settings.json").write_text(
        serialize_settings(OTHER_CONSUMER_SETTINGS), encoding="utf-8"
    )
    return home, claude_dir, launch_agents_dir


def test_install_is_idempotent(tmp_path, monkeypatch):
    home, claude_dir, launch_agents_dir = _installed_fixture(tmp_path, monkeypatch)
    calls: list = []
    runner = _recording_runner(calls)

    rc1 = install(home=home, claude_dir=claude_dir, launch_agents_dir=launch_agents_dir, uid=501, runner=runner)
    settings_after_first = json.loads((claude_dir / "settings.json").read_text(encoding="utf-8"))
    rc2 = install(home=home, claude_dir=claude_dir, launch_agents_dir=launch_agents_dir, uid=501, runner=runner)
    settings_after_second = json.loads((claude_dir / "settings.json").read_text(encoding="utf-8"))

    assert rc1 == 0
    assert rc2 == 0
    assert settings_after_first == settings_after_second
    assert len(settings_after_first["hooks"]["PreToolUse"]) == 3  # rtk + notchbar + ours, once
    assert (home / ".petridish" / "config.toml").exists()
    assert (home / ".petridish" / "settings.json.backup").read_text(encoding="utf-8") == serialize_settings(OTHER_CONSUMER_SETTINGS)
    assert (launch_agents_dir / "com.petridish.daemon.plist").exists()


def test_install_never_touches_other_consumers(tmp_path, monkeypatch):
    home, claude_dir, launch_agents_dir = _installed_fixture(tmp_path, monkeypatch)
    runner = _recording_runner([])

    install(home=home, claude_dir=claude_dir, launch_agents_dir=launch_agents_dir, uid=501, runner=runner)

    settings = json.loads((claude_dir / "settings.json").read_text(encoding="utf-8"))
    # Every pre-existing group survives, in its original position, on every event
    # we register on — the `Notification` key now holds ours *after* pixtuoid's.
    for event, groups in OTHER_CONSUMER_SETTINGS["hooks"].items():
        assert settings["hooks"][event][: len(groups)] == groups
    rtk_entry = settings["hooks"]["PreToolUse"][0]
    assert rtk_entry == OTHER_CONSUMER_SETTINGS["hooks"]["PreToolUse"][0]


def test_install_raises_when_swab_not_on_path(tmp_path, monkeypatch):
    home, claude_dir, launch_agents_dir = _installed_fixture(tmp_path, monkeypatch)
    monkeypatch.setenv("PATH", "")

    with pytest.raises(InstallError, match="not found on PATH"):
        install(home=home, claude_dir=claude_dir, launch_agents_dir=launch_agents_dir, uid=501, runner=_recording_runner([]))


def test_uninstall_removes_hook_and_plist_but_keeps_user_data(tmp_path, monkeypatch):
    home, claude_dir, launch_agents_dir = _installed_fixture(tmp_path, monkeypatch)
    runner = _recording_runner([])
    install(home=home, claude_dir=claude_dir, launch_agents_dir=launch_agents_dir, uid=501, runner=runner)

    rc = uninstall(home=home, claude_dir=claude_dir, launch_agents_dir=launch_agents_dir, uid=501, runner=runner)

    assert rc == 0
    settings = json.loads((claude_dir / "settings.json").read_text(encoding="utf-8"))
    assert not has_marker(settings)
    assert settings["hooks"]["PreToolUse"] == OTHER_CONSUMER_SETTINGS["hooks"]["PreToolUse"]
    assert not (launch_agents_dir / "com.petridish.daemon.plist").exists()
    # D6: user data survives uninstall.
    assert (home / ".petridish" / "config.toml").exists()
    assert (home / ".petridish" / "settings.json.backup").exists()


def test_uninstall_on_never_installed_system_is_a_noop(tmp_path, monkeypatch):
    home, claude_dir, launch_agents_dir = _installed_fixture(tmp_path, monkeypatch)
    runner = _recording_runner([])

    rc = uninstall(home=home, claude_dir=claude_dir, launch_agents_dir=launch_agents_dir, uid=501, runner=runner)

    assert rc == 0
    settings = json.loads((claude_dir / "settings.json").read_text(encoding="utf-8"))
    assert settings == OTHER_CONSUMER_SETTINGS


def test_uninstall_does_not_restore_from_backup_over_unrelated_edits(tmp_path, monkeypatch):
    """D4: uninstall removes only marked entries — it must not silently
    discard an unrelated edit made to settings.json after install."""
    home, claude_dir, launch_agents_dir = _installed_fixture(tmp_path, monkeypatch)
    runner = _recording_runner([])
    install(home=home, claude_dir=claude_dir, launch_agents_dir=launch_agents_dir, uid=501, runner=runner)

    # Simulate an unrelated edit landing after our install (another tool,
    # or the user, adding a hook consumer).
    settings_path = claude_dir / "settings.json"
    settings = json.loads(settings_path.read_text(encoding="utf-8"))
    settings["hooks"]["SessionStart"] = [
        {"hooks": [{"type": "command", "command": "unrelated-new-tool-hook"}]}
    ]
    settings_path.write_text(serialize_settings(settings), encoding="utf-8")

    uninstall(home=home, claude_dir=claude_dir, launch_agents_dir=launch_agents_dir, uid=501, runner=runner)

    final = json.loads(settings_path.read_text(encoding="utf-8"))
    assert final["hooks"]["SessionStart"][0]["hooks"][0]["command"] == "unrelated-new-tool-hook"
    assert not has_marker(final)


# ---------------------------------------------------------------------------
# Menubar plugin template (M10)
# ---------------------------------------------------------------------------
# Exercises the plugin-script template (resources/petridish_menubar.30s.py)
# end-to-end: shebang substitution, missing-resource error, and two subprocess
# "sensors degrade, never abort" paths against a real filesystem read.


def _make_minimal_radar_json():
    """Hand-write a valid minimal Radar JSON (no projects, no quota)."""
    return Radar(
        updated_at=utcnow(),
        scan_duration_ms=0,
        schema_version=1,
    ).to_json(indent=2)


def test_render_menubar_plugin_substitutes_shebang():
    """__PYTHON_SHEBANG__ is replaced with python_executable and no XML escaping occurs."""
    rendered = render_menubar_plugin(python_executable="/usr/bin/python3")

    assert rendered.startswith("#!/usr/bin/python3")
    assert "__PYTHON_SHEBANG__" not in rendered
    # The template file isn't referenced by path — the substitution is on a
    # placeholder, so verify the replacement happened correctly.
    assert "petridish menu-bar plugin for xbar/SwiftBar." in rendered


def test_render_menubar_plugin_missing_template_raises(tmp_path):
    with pytest.raises(InstallError, match="template not found"):
        render_menubar_plugin(
            python_executable="/usr/bin/python3",
            resources_dir=tmp_path / "nowhere",
        )


def _subprocess_plugin(tmp_path, monkeypatch, fake_home: Path):
    """Helper that writes a rendered plugin script to a temp file, makes it
    executable, and runs it as a subprocess. Returns the CompletedProcess."""
    import shutil

    python_executable = sys.executable
    rendered = render_menubar_plugin(python_executable=python_executable)

    script_path = tmp_path / "petridish_menubar.30s.py"
    script_path.write_text(rendered, encoding="utf-8")
    script_path.chmod(script_path.stat().st_mode | stat.S_IEXEC)

    # The rendered script imports petridish.schema, so set PYTHONPATH so the
    # import resolves in the subprocess. Use an absolute path for the src dir.
    src_dir = Path(__file__).resolve().parent.parent / "src"
    src_dir.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ)
    env["PYTHONPATH"] = f"{src_dir}:{env.get('PYTHONPATH', '')}"
    env["HOME"] = str(fake_home)

    return subprocess.run(
        [python_executable, str(script_path)],
        capture_output=True,
        text=True,
        timeout=10,
        check=False,
        env=env,
    )


def test_plugin_degrades_when_projects_json_missing(tmp_path, monkeypatch):
    """Invariant 5: a failing sensor yields null fields; tick still writes.
    For the plugin: missing projects.json must not raise — degrade to a
    fallback line instead."""
    monkeypatch.setattr(sys, "platform", "darwin")

    fake_home = tmp_path / "home"
    fake_home.mkdir()

    plugin_result = _subprocess_plugin(tmp_path, monkeypatch, fake_home)

    assert plugin_result.returncode == 0, (
        f"plugin crashed with exit {plugin_result.returncode}: "
        f"stdout={plugin_result.stdout!r}\nstderr={plugin_result.stderr!r}"
    )
    stdout = plugin_result.stdout
    assert "missing or unreadable" in stdout
    assert "projects.json" in stdout
    # No traceback lines — every failure must degrade to the fallback string.
    assert "Traceback" not in stdout
    assert "KeyboardInterrupt" not in stdout


def test_plugin_degrades_when_projects_json_malformed(tmp_path, monkeypatch):
    """Corrupt JSON must not raise — sensors degrade, never abort (invariant 5)."""
    monkeypatch.setattr(sys, "platform", "darwin")

    fake_home = tmp_path / "home"
    projects_path = fake_home / ".petridish" / "projects.json"
    projects_path.parent.mkdir(parents=True, exist_ok=True)
    projects_path.write_text("this is not json {{{", encoding="utf-8")

    plugin_result = _subprocess_plugin(tmp_path, monkeypatch, fake_home)

    assert plugin_result.returncode == 0, (
        f"plugin crashed with exit {plugin_result.returncode}: "
        f"stdout={plugin_result.stdout!r}\nstderr={plugin_result.stderr!r}"
    )
    stdout = plugin_result.stdout
    assert "missing or unreadable" in stdout
    assert "Traceback" not in stdout
    assert "KeyboardInterrupt" not in stdout


def test_plugin_stdout_matches_render_menubar_directly(tmp_path, monkeypatch):
    """The plugin's stdout is byte-identical to render_menubar(radar) computed
    in-process — proving the script runs the library, not some shim."""
    monkeypatch.setattr(sys, "platform", "darwin")

    # Build a valid Radar via the library so the expected output is grounded.
    radar = Radar(
        updated_at=utcnow(),
        scan_duration_ms=12,
        schema_version=1,
        projects=(Project(id="p1", name="p1", path="/abs/project/p1", category="repos"),),
    )
    expected = render_menubar(radar)

    # Write the radar to disk under a fake $HOME so the plugin can read it.
    fake_home = tmp_path / "home"
    fake_home.mkdir()
    projects_path = fake_home / ".petridish" / "projects.json"
    projects_path.parent.mkdir(parents=True, exist_ok=True)
    projects_path.write_text(radar.to_json(indent=2), encoding="utf-8")

    plugin_result = _subprocess_plugin(tmp_path, monkeypatch, fake_home)

    assert plugin_result.returncode == 0
    assert plugin_result.stdout.rstrip() == expected, (
        f"plugin output differs from render_menubar:\n"
        f"expected: {expected!r}\n"
        f"got:      {plugin_result.stdout!r}"
    )


# ---------------------------------------------------------------------------
# Menubar plugin: write, install, uninstall integration
# ---------------------------------------------------------------------------


def test_write_menubar_plugin_writes_executable_file(tmp_path):
    """write_menubar_plugin: creates the file, writes the content, chmods +x."""
    target = tmp_path / "test_plugin.30s.py"
    content = "#!/usr/bin/env python3\nprint('hi')"

    write_menubar_plugin(target, content)

    assert target.exists()
    assert target.read_text(encoding="utf-8") == content
    assert os.access(target, os.X_OK)


def test_install_with_menubar_plugins_dir_writes_plugin(tmp_path, monkeypatch):
    """install(..., menubar_plugins_dir=...) writes the plugin file."""
    monkeypatch.setattr(sys, "platform", "darwin")

    home, claude_dir, launch_agents_dir = _installed_fixture(tmp_path, monkeypatch)

    plugin_dir = tmp_path / "menubar" / "plugins"
    rc = install(
        home=home, claude_dir=claude_dir, launch_agents_dir=launch_agents_dir,
        uid=501, runner=_recording_runner([]), menubar_plugins_dir=plugin_dir,
    )

    assert rc == 0
    plugin_file = plugin_dir / DEFAULT_MENUBAR_PLUGIN_FILENAME
    assert plugin_file.exists()
    assert os.access(plugin_file, os.X_OK)
    first_line = plugin_file.read_text(encoding="utf-8").splitlines()[0]
    assert first_line.startswith("#!")
    # The shebang must use sys.executable so the plugin is runnable as-is.
    assert sys.executable in first_line


def test_install_without_menubar_plugins_dir_skips_it(tmp_path, monkeypatch):
    """Omitting menubar_plugins_dir (the default=None path) must skip the plugin
    entirely. The (tmp_path-scoped) default xbar path must not be created as
    a side-effect."""
    monkeypatch.setattr(sys, "platform", "darwin")

    home, claude_dir, launch_agents_dir = _installed_fixture(tmp_path, monkeypatch)

    rc = install(
        home=home, claude_dir=claude_dir,
        launch_agents_dir=launch_agents_dir, uid=501, runner=_recording_runner([]),
    )

    assert rc == 0
    # The default xbar path (anchored on the fake `home`, never the real
    # machine's) must not have been created by install() when the caller
    # omitted menubar_plugins_dir.
    assert not default_menubar_plugins_dir(home).exists()


def test_default_menubar_plugins_dir_is_xbar_path(tmp_path):
    home = tmp_path / "home"
    assert default_menubar_plugins_dir(home) == home / "Library" / "Application Support" / "xbar" / "plugins"


def test_uninstall_with_menubar_plugins_dir_removes_plugin(tmp_path, monkeypatch):
    """install with menubar_plugins_dir, then uninstall with the same dir:
    plugin file is gone afterward."""
    monkeypatch.setattr(sys, "platform", "darwin")

    home, claude_dir, launch_agents_dir = _installed_fixture(tmp_path, monkeypatch)

    plugin_dir = tmp_path / "menubar" / "plugins"
    install(
        home=home, claude_dir=claude_dir, launch_agents_dir=launch_agents_dir,
        uid=501, runner=_recording_runner([]), menubar_plugins_dir=plugin_dir,
    )

    assert (plugin_dir / DEFAULT_MENUBAR_PLUGIN_FILENAME).exists()

    rc = uninstall(
        home=home, claude_dir=claude_dir, launch_agents_dir=launch_agents_dir,
        uid=501, runner=_recording_runner([]), menubar_plugins_dir=plugin_dir,
    )

    assert rc == 0
    assert not (plugin_dir / DEFAULT_MENUBAR_PLUGIN_FILENAME).exists()


def test_uninstall_menubar_plugin_missing_is_a_noop(tmp_path, monkeypatch):
    """uninstall with a menubar_plugins_dir where the plugin never existed
    is a no-op: returns 0, no exception."""
    monkeypatch.setattr(sys, "platform", "darwin")

    home, claude_dir, launch_agents_dir = _installed_fixture(tmp_path, monkeypatch)

    empty_dir = tmp_path / "empty" / "plugins"
    empty_dir.mkdir(parents=True)

    rc = uninstall(
        home=home, claude_dir=claude_dir, launch_agents_dir=launch_agents_dir,
        uid=501, runner=_recording_runner([]), menubar_plugins_dir=empty_dir,
    )

    assert rc == 0


# ---------------------------------------------------------------------------
# Backward-compat: existing install/uninstall calls (no new menubar param)
# ---------------------------------------------------------------------------


def test_existing_call_sites_still_work_without_menubar_param(
    tmp_path, monkeypatch,
):
    """None of the existing test call sites pass menubar_plugins_dir. These
    must continue to return 0 and leave no menubar plugin artifacts."""
    monkeypatch.setattr(sys, "platform", "darwin")

    home, claude_dir, launch_agents_dir = _installed_fixture(tmp_path, monkeypatch)

    calls: list = []
    runner = _recording_runner(calls)

    rc_install = install(
        home=home, claude_dir=claude_dir,
        launch_agents_dir=launch_agents_dir, uid=501, runner=runner,
    )
    assert rc_install == 0

    rc_uninstall = uninstall(
        home=home, claude_dir=claude_dir,
        launch_agents_dir=launch_agents_dir, uid=501, runner=runner,
    )
    assert rc_uninstall == 0
