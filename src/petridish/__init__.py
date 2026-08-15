"""petridish — local monitoring daemon for macOS.

The scanner that crawls project roots, tracks git state and AI-agent activity, and writes
``~/.petridish/projects.json`` is `swab` (Rust, ``swab/`` at the repo root) — see
``swab/src/git.rs``'s module doc comment for why (gix beats both a CLI-subprocess and a
git2 backend on real measurements). This package is now the **read side** only: `petridish.schema`
is the shared contract every frontend (`petri`, `menubar.py`) parses ``projects.json``
through, and `petridish.installer` wires up the launchd job + Claude Code hook that invoke
the Rust binaries.
"""

__version__ = "0.1.0"
