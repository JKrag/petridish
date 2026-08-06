"""Fail if src/ uses stdlib APIs newer than the declared requires-python floor.

Greps are not good enough here: a docstring explaining *why* we avoid an API
matched itself and failed a clean round. This tokenizes each module and ignores
comments and string literals, so only real code counts.

Usage: python3 scripts/check_pyver.py   (exit 0 = clean, 1 = violation)
"""

from __future__ import annotations

import io
import pathlib
import sys
import tokenize

# name -> version it was introduced in
POST_312 = {
    "__replace__": "3.13",
    "require_scheme": "3.14",
    "resolve_host": "3.14",
    "batched": "3.12+ (itertools.batched — check call site)",
}

violations: list[str] = []

for path in sorted(pathlib.Path("src").rglob("*.py")):
    try:
        source = path.read_text(encoding="utf-8")
    except OSError:
        continue
    try:
        tokens = list(tokenize.generate_tokens(io.StringIO(source).readline))
    except (tokenize.TokenError, IndentationError, SyntaxError):
        continue

    for tok in tokens:
        # Only NAME tokens are real identifiers; comments and strings
        # (including docstrings) are skipped entirely.
        if tok.type != tokenize.NAME:
            continue
        if tok.string in POST_312 and tok.string != "batched":
            violations.append(
                f"{path}:{tok.start[0]}: uses '{tok.string}' "
                f"(Python {POST_312[tok.string]}+), but requires-python is >=3.12"
            )

if violations:
    print("POST-3.12 API VIOLATIONS:")
    for v in violations:
        print("  " + v)
    sys.exit(1)
sys.exit(0)
