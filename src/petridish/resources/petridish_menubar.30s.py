#!__PYTHON_SHEBANG__
"""petridish menu-bar plugin for xbar/SwiftBar.

Reads ~/.petridish/projects.json and prints the xbar/SwiftBar plugin-text via
petridish.menubar.render_menubar. Must never crash or exit non-zero -- xbar
disables a plugin that errors, so every failure mode degrades to a plain
fallback line instead of raising.
"""
from __future__ import annotations

from pathlib import Path


def main() -> int:
    try:
        from petridish.menubar import render_menubar
        from petridish.schema import read_json
    except ImportError:
        print("petridish package not importable | color=#ff0000")
        return 0

    projects_path = Path.home() / ".petridish" / "projects.json"
    try:
        radar = read_json(projects_path)
        output = render_menubar(radar)
    except Exception:
        print("🧫 ?/?")
        print("---")
        print(f"projects.json missing or unreadable ({projects_path}) | color=#888888")
        print("---")
        print("Refresh | refresh=true")
        return 0

    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
