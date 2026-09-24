"""Tests for petridish-tray's xbar-text parser, mirroring the Cinnamon
applet's parser tests (integrations/cinnamon/tests/parser.test.js) — the two
parse the same menubar.rs output and must agree. Plain unittest, no deps:

    python3 integrations/sni-tray/tests/test_parser.py
"""
import importlib.util
import pathlib
import sys
import unittest

SCRIPT = pathlib.Path(__file__).resolve().parent.parent / "petridish-tray"


def load_parser():
    """Load the script's parse functions without running its main loop:
    execute only the source above `class Tray`, which also skips the gi
    import — the parser itself is dependency-free on purpose."""
    src = SCRIPT.read_text()
    module_src = src.split("class Tray")[0]
    module_src = module_src.replace("from gi.repository import Gio, GLib", "")
    ns = {}
    exec(compile(module_src, str(SCRIPT), "exec"), ns)
    return ns


P = load_parser()


class ParserTests(unittest.TestCase):
    def test_empty_radar_output(self):
        title, lines = P["parse_menubar_text"](
            "🧫 0/0\n---\nNo projects | color=#888888\n---\nRefresh | refresh=true")
        self.assertEqual(title, "🧫 0/0")
        self.assertEqual(lines[0]["params"]["color"], "#888888")
        self.assertEqual(lines[1]["kind"], "separator")
        self.assertEqual(lines[2]["params"]["refresh"], "true")

    def test_bucket_header_and_indented_member(self):
        _, lines = P["parse_menubar_text"](
            '🧫 0/1\n---\nActive\n--active-one · main | href="file:///x/active-one"\n---\nRefresh | refresh=true')
        self.assertEqual(lines[0], {"kind": "item", "text": "Active", "indent": False, "params": {}})
        self.assertTrue(lines[1]["indent"])
        self.assertEqual(lines[1]["params"]["href"], "file:///x/active-one")

    def test_quoted_href_keeps_spaces(self):
        p = P["parse_params"]('href="file:///x/Kubernetes handin_639180485"')
        self.assertEqual(p["href"], "file:///x/Kubernetes handin_639180485")

    def test_multiple_params(self):
        p = P["parse_params"]('href="file:///x" color=#ff0000 refresh=true')
        self.assertEqual(p, {"href": "file:///x", "color": "#ff0000", "refresh": "true"})

    def test_live_session_line(self):
        line = P["parse_line"]('live-one · main ● | href="file:///x/live-one"')
        self.assertFalse(line["indent"])
        self.assertEqual(line["text"], "live-one · main ●")

    def test_empty_input_degrades(self):
        for text in ("", None):
            title, lines = P["parse_menubar_text"](text)
            self.assertEqual(title, "🧫 ?")
            self.assertEqual(lines, [])

    def test_separator_vs_dirty_marker(self):
        self.assertEqual(P["parse_line"]("---"), {"kind": "separator"})
        dirty = P["parse_line"]('--p · main ✎3 | href="file:///x/p"')
        self.assertEqual(dirty["text"], "p · main ✎3")

    def test_malformed_param_tail(self):
        line = P["parse_line"]("something | notaparam")
        self.assertEqual(line["text"], "something")
        self.assertEqual(line["params"], {})


if __name__ == "__main__":
    unittest.main()
