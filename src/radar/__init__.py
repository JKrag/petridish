"""project-radar — local monitoring daemon for macOS.

Crawls configured project roots, tracks git state and AI-agent activity, and
aggregates into ``~/.project-radar/projects.json``.

Public surface is intentionally small: ``load_config`` in
``radar.config`` is the only entry point for the daemon core; everything else
is consumed from ``radar.scan``. The CLI and the hook are thin wrappers written
in later modules (M8, M6).
"""

__version__ = "0.1.0"
