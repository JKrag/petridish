// Parser for xbar/SwiftBar plugin text, the format `petridish menubar` prints.
// Pure: no Cinnamon imports, no I/O — so the same file loads both inside
// Cinnamon (via imports.ui.appletManager.applets) and under node's test runner
// (via the module.exports guard at the bottom).
//
// The subset parsed here is exactly what menubar.rs emits, documented in its
// module comment: a title line, `---` separators, `--`-indented lines, and
// ` | key=value` parameters where only `href` carries a quoted value. It is
// not a general xbar parser and doesn't try to be one.

/**
 * Split the parameter tail of a line (everything after ` | `) into a map.
 * Values may be double-quoted; a quoted value keeps its inner whitespace,
 * which is the whole reason menubar.rs quotes `href` (paths with spaces).
 */
function parseParams(tail) {
    const params = {};
    let i = 0;
    while (i < tail.length) {
        while (tail[i] === " ") i++;
        if (i >= tail.length) break;
        const eq = tail.indexOf("=", i);
        if (eq === -1) break; // malformed tail: ignore the rest, keep the line usable
        const key = tail.slice(i, eq);
        let value;
        if (tail[eq + 1] === '"') {
            const close = tail.indexOf('"', eq + 2);
            if (close === -1) {
                value = tail.slice(eq + 2); // unterminated quote: take the rest
                i = tail.length;
            } else {
                value = tail.slice(eq + 2, close);
                i = close + 1;
            }
        } else {
            let end = tail.indexOf(" ", eq + 1);
            if (end === -1) end = tail.length;
            value = tail.slice(eq + 1, end);
            i = end;
        }
        params[key] = value;
    }
    return params;
}

/**
 * One parsed line: { kind: "separator" } or
 * { kind: "item", text, indent, params } where indent is true for `--` lines.
 */
function parseLine(line) {
    if (line === "---") return { kind: "separator" };
    let indent = false;
    let rest = line;
    if (rest.startsWith("--")) {
        indent = true;
        rest = rest.slice(2);
    }
    const sep = rest.indexOf(" | ");
    if (sep === -1) {
        return { kind: "item", text: rest, indent, params: {} };
    }
    return {
        kind: "item",
        text: rest.slice(0, sep),
        indent,
        params: parseParams(rest.slice(sep + 3)),
    };
}

/**
 * Whole plugin text -> { title, lines }. `title` is the first line's text
 * (panel label); `lines` is every parsed line after the first `---`, in
 * order, for the dropdown. Empty input degrades to a "?" title so the
 * applet never renders an empty panel slot.
 */
function parseMenubarText(text) {
    const raw = (text || "").replace(/\n+$/, "").split("\n");
    if (raw.length === 0 || raw[0] === "") {
        return { title: "🧫 ?", lines: [] };
    }
    const title = raw[0];
    // Everything after the first separator is dropdown content. If there is
    // no separator at all (not a shape menubar.rs produces), show just the title.
    const firstSep = raw.indexOf("---");
    const body = firstSep === -1 ? [] : raw.slice(firstSep + 1);
    return { title, lines: body.map(parseLine) };
}

if (typeof module !== "undefined" && module.exports) {
    module.exports = { parseParams, parseLine, parseMenubarText };
}
