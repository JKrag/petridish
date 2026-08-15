#!/usr/bin/env python3
"""Structural, field-masked diff of two projects.json-shaped files.

Used by diff_check.sh to compare Python swab's and Rust swab's output for the same
fixture $HOME. Ignores key order (both are parsed to Python objects, not text-diffed) and
masks fields that are legitimately nondeterministic between two separate runs/processes:
`updated_at`, `scan_duration_ms`, and any field literally named `*mtime*`.

Exit 0 and print nothing on a match. Exit 1 and print exactly what diverged on a mismatch.
"""
from __future__ import annotations

import json
import sys
from typing import Any

MASKED_TOP_LEVEL = {"updated_at", "scan_duration_ms"}


def _mask(value: Any) -> Any:
    if isinstance(value, dict):
        return {
            k: ("<masked>" if k in MASKED_TOP_LEVEL or "mtime" in k else _mask(v))
            for k, v in value.items()
        }
    if isinstance(value, list):
        return [_mask(v) for v in value]
    return value


def _diff(a: Any, b: Any, path: str, out: list[str]) -> None:
    if isinstance(a, dict) and isinstance(b, dict):
        for key in sorted(set(a) | set(b)):
            if key not in a:
                out.append(f"{path}.{key}: missing on left, present on right ({b[key]!r})")
            elif key not in b:
                out.append(f"{path}.{key}: present on left ({a[key]!r}), missing on right")
            else:
                _diff(a[key], b[key], f"{path}.{key}", out)
    elif isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            out.append(f"{path}: length {len(a)} vs {len(b)}")
        for i, (av, bv) in enumerate(zip(a, b)):
            _diff(av, bv, f"{path}[{i}]", out)
    elif a != b:
        out.append(f"{path}: {a!r} vs {b!r}")


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: compare_radar.py <python-output.json> <rust-output.json>", file=sys.stderr)
        return 2

    left_path, right_path = sys.argv[1], sys.argv[2]
    with open(left_path) as f:
        left = _mask(json.load(f))
    with open(right_path) as f:
        right = _mask(json.load(f))

    # Project order is a defined sort (last_activity_at desc, then name asc) — compare as
    # lists in that order rather than re-sorting, so a sort-order regression is caught too.
    diffs: list[str] = []
    _diff(left, right, "$", diffs)

    if diffs:
        print(f"MISMATCH ({len(diffs)} field(s) diverged):", file=sys.stderr)
        for d in diffs:
            print(f"  {d}", file=sys.stderr)
        return 1

    print("MATCH")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
