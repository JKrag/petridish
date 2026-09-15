// Tests for the applet's xbar-text parser, run with node's built-in runner
// (`node --test`) so they need no npm install — mirroring how the parser file
// itself needs no Cinnamon to load. The fixture strings are the exact shapes
// menubar.rs's own tests assert it produces.

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { parseParams, parseLine, parseMenubarText } = require("../petridish@jkrag/parser.js");

test("an empty radar's output parses to title + placeholder + refresh", () => {
    const out = parseMenubarText(
        "🧫 0/0\n---\nNo projects | color=#888888\n---\nRefresh | refresh=true"
    );
    assert.equal(out.title, "🧫 0/0");
    assert.deepEqual(out.lines, [
        { kind: "item", text: "No projects", indent: false, params: { color: "#888888" } },
        { kind: "separator" },
        { kind: "item", text: "Refresh", indent: false, params: { refresh: "true" } },
    ]);
});

test("a bucket section parses as a bare header plus indented members", () => {
    const out = parseMenubarText(
        "🧫 0/1\n---\nActive\n--active-one · main | href=\"file:///Users/x/repos/active-one\"\n---\nRefresh | refresh=true"
    );
    assert.deepEqual(out.lines[0], { kind: "item", text: "Active", indent: false, params: {} });
    assert.deepEqual(out.lines[1], {
        kind: "item",
        text: "active-one · main",
        indent: true,
        params: { href: "file:///Users/x/repos/active-one" },
    });
});

test("a quoted href keeps its inner spaces", () => {
    // The path that found xbar's own quoting bug — menubar.rs quotes href for it.
    const p = parseParams('href="file:///Users/x/Downloads/Kubernetes handin_639180485"');
    assert.equal(p.href, "file:///Users/x/Downloads/Kubernetes handin_639180485");
});

test("multiple parameters on one line all parse", () => {
    const p = parseParams('href="file:///x" color=#ff0000 refresh=true');
    assert.deepEqual(p, { href: "file:///x", color: "#ff0000", refresh: "true" });
});

test("a live-session line is top-level with an href", () => {
    const line = parseLine('live-one · main ● | href="file:///Users/x/repos/live-one"');
    assert.equal(line.indent, false);
    assert.equal(line.text, "live-one · main ●");
    assert.equal(line.params.href, "file:///Users/x/repos/live-one");
});

test("the schema-drift warning line carries its color through", () => {
    const line = parseLine(
        "projects.json schema (v99) is newer than this build supports (v1); upgrade petridish/swab/petri together | color=#ff0000"
    );
    assert.equal(line.params.color, "#ff0000");
    assert.match(line.text, /newer than this build supports/);
});

test("empty and missing input degrade to a placeholder title, never a crash", () => {
    for (const input of ["", null, undefined]) {
        const out = parseMenubarText(input);
        assert.equal(out.title, "🧫 ?");
        assert.deepEqual(out.lines, []);
    }
});

test("a separator line is distinct from a dirty-marker project line", () => {
    assert.deepEqual(parseLine("---"), { kind: "separator" });
    const dirty = parseLine('--p · main ✎3 | href="file:///x/p"');
    assert.equal(dirty.kind, "item");
    assert.equal(dirty.text, "p · main ✎3");
});

test("a malformed parameter tail keeps the line text usable", () => {
    const line = parseLine("something | notaparam");
    assert.equal(line.text, "something");
    assert.deepEqual(line.params, {});
});
