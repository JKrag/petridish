"""Every character petri can put on a terminal must actually render there.

Learned the expensive way. ``⚠`` (U+26A0, added in Unicode 4.0) came out as a
**blank cell** on the macOS 14 CI runner, while ``●`` (U+25CF) and ``✎``
(U+270E) — both Unicode 1.1 — rendered fine. ncurses asks ``wcwidth()`` before
placing a character, macOS's tables lag the standard, and for a codepoint they
don't know the answer is -1, so ncurses substitutes a space.

What made it expensive is what it broke: the stalled-run glyph is, by the
dashboard's own design, *the row you opened it to find*. It silently vanished,
and nothing in a 300-test suite noticed, because every project's name also
appears on its path row — so every substring assertion still passed and the
screen read as a calm dashboard.

Hence a mechanical gate rather than a convention. Any non-ASCII character
introduced into a curses-side module fails this test until it is added below
with a reason. **The bar for adding one: it must exist in Unicode 1.1 (1993)** —
old enough that every ``wcwidth`` table still in service knows it. A prettier
glyph is not worth a warning that only works on the author's laptop.

Scope note: this deliberately scans whole files, comments and docstrings
included, rather than trying to guess statically which literals reach the
screen. A stray unportable character in a comment is a loaded gun — the ``⏎``
that used to sit in ``screens.py``'s footer comment was one keystroke from
being pasted into a real footer.

``menubar.py`` is out of scope: SwiftBar renders it, not ncurses, and its 🧫
(Unicode 11.0) is fine there.
"""

from __future__ import annotations

import pathlib

#: Modules whose output ncurses draws.
CURSES_MODULES = ("tui.py", "tui_state.py", "screens.py")

#: Every non-ASCII character permitted in the modules above, with the Unicode
#: version that introduced it. All 1.1 or earlier; see the module docstring for
#: why that is the bar. Confirmed rendering on the macOS 14 CI runner.
ALLOWED = {
    "·": ("U+00B7", "1.1", "separator in card and header lines"),
    "—": ("U+2014", "1.1", "em dash, prose only"),
    "…": ("U+2026", "1.1", "truncation marker"),
    "─": ("U+2500", "1.1", "section rule"),
    "│": ("U+2502", "1.1", "browser pane divider"),
    "└": ("U+2514", "1.1", "browser pane corner"),
    "┴": ("U+2534", "1.1", "browser pane tee"),
    "═": ("U+2550", "1.1", "header rule"),
    "╤": ("U+2564", "1.1", "browser pane tee"),
    "█": ("U+2588", "1.1", "quota bar, filled"),
    "░": ("U+2591", "1.1", "quota bar, empty"),
    "▲": ("U+25B2", "1.1", "agent glyph: stalled. Replaced ⚠ U+26A0 (4.0), "
                            "which rendered as a blank cell on macOS 14."),
    "○": ("U+25CB", "1.1", "agent glyph: idle or finished"),
    "●": ("U+25CF", "1.1", "agent glyph: working"),
    "✎": ("U+270E", "1.1", "uncommitted-files marker"),
}

SRC = pathlib.Path(__file__).resolve().parent.parent / "src" / "petridish"


def test_curses_modules_use_only_portable_characters():
    """No non-ASCII character outside ALLOWED may appear in a curses module."""
    offenders: list[str] = []
    for name in CURSES_MODULES:
        text = (SRC / name).read_text(encoding="utf-8")
        for lineno, line in enumerate(text.splitlines(), 1):
            for ch in line:
                if ord(ch) > 0x7F and ch not in ALLOWED:
                    offenders.append(f"{name}:{lineno} U+{ord(ch):04X} {ch!r}")

    assert not offenders, (
        "Unportable character(s) in curses-rendered code. ncurses substitutes a "
        "blank for any codepoint macOS's wcwidth tables don't know, so this "
        "would render as an invisible gap on some supported machines:\n  "
        + "\n  ".join(sorted(set(offenders)))
        + "\n\nIf the character genuinely exists in Unicode 1.1, add it to "
          "ALLOWED with a reason. Otherwise pick an older equivalent."
    )


def test_allowlist_entries_are_actually_old_enough():
    """The allowlist's own claim is checked, not trusted.

    An allowlist whose entries nobody verifies is just a bigger version of the
    bug. Every codepoint here must sit in a block that predates Unicode 2.0:
    Latin-1 Supplement, General Punctuation, Box Drawing, Block Elements,
    Geometric Shapes, or Dingbats.
    """
    old_blocks = (
        (0x0080, 0x00FF),  # Latin-1 Supplement
        (0x2000, 0x206F),  # General Punctuation
        (0x2500, 0x257F),  # Box Drawing
        (0x2580, 0x259F),  # Block Elements
        (0x25A0, 0x25FF),  # Geometric Shapes
        (0x2700, 0x27BF),  # Dingbats
    )
    for ch, (code, version, why) in ALLOWED.items():
        cp = ord(ch)
        assert any(lo <= cp <= hi for lo, hi in old_blocks), (
            f"{ch!r} ({code}, claimed {version}, {why}) is not in a "
            "pre-Unicode-2.0 block — the claim is unverified, so it must not be "
            "on this list."
        )
        assert code == f"U+{cp:04X}", f"{ch!r} is labelled {code} but is U+{cp:04X}"
