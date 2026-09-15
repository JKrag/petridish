// petridish Cinnamon applet: the Linux counterpart of the xbar/SwiftBar
// plugin. A thin presenter over one command — `petridish menubar` — whose
// output shape is owned by menubar.rs; this file only parses and displays it,
// so the two frontends can't drift apart on content.

const Applet = imports.ui.applet;
const PopupMenu = imports.ui.popupMenu;
const Settings = imports.ui.settings;
const GLib = imports.gi.GLib;
const Gio = imports.gi.Gio;

const UUID = "petridish@jkrag";
const Parser = imports.ui.appletManager.applets[UUID].parser;

// Same problem xbar has on macOS (see integrations/xbar/README.md): Cinnamon
// is launched by the session manager, not a shell, so ~/.cargo/bin and
// ~/.local/bin are usually not on its PATH. Look there explicitly rather than
// requiring the user to know that.
function findBinary(configured) {
    if (configured && configured.length > 0) return configured;
    const onPath = GLib.find_program_in_path("petridish");
    if (onPath) return onPath;
    const home = GLib.get_home_dir();
    const candidates = [home + "/.cargo/bin/petridish", home + "/.local/bin/petridish"];
    for (const c of candidates) {
        if (GLib.file_test(c, GLib.FileTest.IS_EXECUTABLE)) return c;
    }
    return null;
}

class PetridishApplet extends Applet.TextIconApplet {
    constructor(metadata, orientation, panelHeight, instanceId) {
        super(orientation, panelHeight, instanceId);

        // The title line starts with 🧫, but St panel labels render color emoji
        // as a tofu box — so the panel shows a shipped icon instead and the
        // emoji is stripped from the label in _render. The PNG, not the SVG:
        // verified live on Cinnamon 6.6 that a Gio.FileIcon over the SVG paints
        // nothing (silently), while the same file pre-rendered to PNG shows up.
        // icon.svg stays in the tree as the editable source of icon.png.
        this.set_applet_icon_path(metadata.path + "/icon.png");
        this.set_applet_label("…");
        this.set_applet_tooltip("petridish — project fleet");

        this.menuManager = new PopupMenu.PopupMenuManager(this);
        this.menu = new Applet.AppletPopupMenu(this, orientation);
        this.menuManager.addMenu(this.menu);

        this.settings = new Settings.AppletSettings(this, UUID, instanceId);
        this.settings.bind("refresh-seconds", "refreshSeconds", () => this._restartTimer());
        this.settings.bind("binary-path", "binaryPath", () => this._refresh());

        this._timerId = 0;
        this._refresh();
        this._restartTimer();
    }

    on_applet_clicked() {
        this.menu.toggle();
    }

    on_applet_removed_from_panel() {
        this._stopTimer();
        this.settings.finalize();
    }

    _stopTimer() {
        if (this._timerId) {
            GLib.source_remove(this._timerId);
            this._timerId = 0;
        }
    }

    _restartTimer() {
        this._stopTimer();
        this._timerId = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, this.refreshSeconds, () => {
            this._refresh();
            return GLib.SOURCE_CONTINUE;
        });
    }

    _refresh() {
        const bin = findBinary(this.binaryPath);
        if (!bin) {
            this._render(
                "🧫 ?",
                [
                    {
                        kind: "item",
                        text: "petridish binary not found (PATH, ~/.cargo/bin, ~/.local/bin) — set it in the applet settings",
                        indent: false,
                        params: { color: "#888888" },
                    },
                ]
            );
            return;
        }
        let proc;
        try {
            proc = Gio.Subprocess.new(
                [bin, "menubar"],
                Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_PIPE
            );
        } catch (e) {
            this._render("🧫 ?", [
                { kind: "item", text: `failed to run ${bin}: ${e}`, indent: false, params: { color: "#888888" } },
            ]);
            return;
        }
        proc.communicate_utf8_async(null, null, (p, res) => {
            let stdout = "";
            try {
                [, stdout] = p.communicate_utf8_finish(res);
            } catch (e) {
                // Degrade, never blank the panel — same contract as the xbar
                // plugin ("always exits 0, always prints something").
                this._render("🧫 ?", [
                    { kind: "item", text: `petridish menubar failed: ${e}`, indent: false, params: { color: "#888888" } },
                ]);
                return;
            }
            const parsed = Parser.parseMenubarText(stdout);
            this._render(parsed.title, parsed.lines);
        });
    }

    _render(title, lines) {
        this.set_applet_label(title.replace(/^🧫\s*/, ""));
        this.menu.removeAll();

        for (const line of lines) {
            if (line.kind === "separator") {
                this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
                continue;
            }
            // `Refresh | refresh=true` is xbar's manual-refresh affordance;
            // here it re-runs the command instead of re-running a plugin file.
            if (line.params.refresh === "true") {
                const refreshItem = new PopupMenu.PopupMenuItem(line.text);
                refreshItem.connect("activate", () => this._refresh());
                this.menu.addMenuItem(refreshItem);
                continue;
            }
            const href = line.params.href;
            // A top-level line with no parameters is a bucket header
            // ("Active", "In flight", ...) in menubar.rs's output shape.
            const isHeader = !line.indent && !href && Object.keys(line.params).length === 0;
            const item = new PopupMenu.PopupMenuItem(
                line.indent ? "    " + line.text : line.text,
                { reactive: Boolean(href) }
            );
            if (isHeader) item.label.set_style("font-weight: bold;");
            if (line.params.color) item.label.set_style(`color: ${line.params.color};`);
            if (href) {
                item.connect("activate", () => {
                    try {
                        Gio.AppInfo.launch_default_for_uri(href, null);
                    } catch (e) {
                        global.logError(`${UUID}: could not open ${href}: ${e}`);
                    }
                });
            }
            this.menu.addMenuItem(item);
        }
    }
}

function main(metadata, orientation, panelHeight, instanceId) {
    return new PetridishApplet(metadata, orientation, panelHeight, instanceId);
}
