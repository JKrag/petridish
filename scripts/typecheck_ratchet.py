"""Fail if pyright errors under ``src/`` grew above the committed baseline.

Why a ratchet instead of "pyright must be clean": the annotations in this
project predate anything checking them, so turning pyright on surfaced a few
hundred pre-existing diagnostics at once. A gate demanding zero would be red on
every push from day one, which trains everyone to ignore it — the worst
possible outcome for a check meant to catch regressions.

So the gate is directional: the count may fall, never rise. Fix a file, lower
the number, commit it. That makes the baseline a visible debt counter rather
than a permanently-failing build.

``tests/`` is deliberately excluded from the count. pytest fixtures
(``tmp_path``, ``monkeypatch``, ``capsys``) are untyped at the call site, so
strict mode reports every fixture parameter and everything derived from one —
roughly a thousand diagnostics with no signal in them.

Usage:  python3 scripts/typecheck_ratchet.py   (exit 0 = clean or improved)
        python3 scripts/typecheck_ratchet.py --update   (rewrite the baseline)
"""

from __future__ import annotations

import collections
import json
import pathlib
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
BASELINE_FILE = REPO / "typecheck-baseline.txt"


def count_src_errors() -> tuple[int, collections.Counter[str]]:
    """Return (total, per-file counts) of error-severity diagnostics in src/."""
    proc = subprocess.run(
        ["pyright", "--outputjson"],
        cwd=REPO,
        capture_output=True,
        text=True,
        check=False,  # pyright exits non-zero whenever diagnostics exist
    )
    if not proc.stdout.strip():
        print("typecheck-ratchet: pyright produced no output", file=sys.stderr)
        print(proc.stderr[-2000:], file=sys.stderr)
        raise SystemExit(2)
    try:
        data = json.loads(proc.stdout)
    except json.JSONDecodeError:
        print("typecheck-ratchet: could not parse pyright JSON", file=sys.stderr)
        print(proc.stdout[:2000], file=sys.stderr)
        raise SystemExit(2)

    per_file: collections.Counter[str] = collections.Counter()
    for diag in data.get("generalDiagnostics", []):
        path = diag.get("file", "")
        if "/src/" in path and diag.get("severity") == "error":
            per_file[pathlib.Path(path).name] += 1
    return sum(per_file.values()), per_file


def read_baseline() -> int:
    if not BASELINE_FILE.exists():
        print(
            f"typecheck-ratchet: no baseline at {BASELINE_FILE.name}; "
            "create it with --update",
            file=sys.stderr,
        )
        raise SystemExit(2)
    return int(BASELINE_FILE.read_text().split("#")[0].strip())


def write_baseline(n: int) -> None:
    BASELINE_FILE.write_text(
        f"{n}\n"
        "# pyright error-severity diagnostics under src/. Lower is better; this\n"
        "# gate fails if the number RISES. Improve a file, then run\n"
        "#   python3 scripts/typecheck_ratchet.py --update\n"
        "# and commit the smaller number alongside the fix.\n"
    )


def main() -> int:
    update = "--update" in sys.argv
    total, per_file = count_src_errors()

    if update:
        write_baseline(total)
        print(f"typecheck-ratchet: baseline set to {total}")
        return 0

    baseline = read_baseline()

    if total > baseline:
        print(f"typecheck-ratchet: FAIL — src errors rose {baseline} -> {total}")
        print("Worst files:")
        for name, n in per_file.most_common(5):
            print(f"  {n:5d}  {name}")
        print("\nRun `uv run --extra dev pyright` to see them.")
        return 1

    if total < baseline:
        print(
            f"typecheck-ratchet: improved {baseline} -> {total}. "
            "Run with --update and commit the new baseline."
        )
        return 0

    print(f"typecheck-ratchet: OK — {total} src errors (baseline {baseline})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
