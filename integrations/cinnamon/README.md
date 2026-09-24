# Panel applet (Cinnamon)

The Linux counterpart of the [xbar/SwiftBar menu bar plugin](../xbar/README.md):
`🧫 working/total` in the Cinnamon panel, running sessions at the top of the
dropdown, the rest grouped by bucket. Clicking a project opens its directory in
the file manager.

Like the xbar plugin, it is a thin presenter over one command:

```sh
petridish menubar
```

The applet runs that every 30 seconds (configurable), parses the output, and
renders it. All content decisions — labels, ordering, the dirty/working
markers — live in `menubar.rs`, shared with macOS, so the two menu bars cannot
drift apart.

## Install

```sh
cp -r integrations/cinnamon/petridish@jkrag ~/.local/share/cinnamon/applets/
```

Then right-click the panel → **Applets** → find **petridish** → add it. No
Cinnamon restart needed; newly copied applets appear in the list immediately.

The applet needs the `petridish` binary. It searches `PATH`, then
`~/.cargo/bin`, then `~/.local/bin` — the same not-launched-from-a-shell
problem xbar has on macOS, solved by searching rather than by generating a
wrapper. If your binary lives somewhere else, set the path in the applet's
settings (right-click the applet → **Configure**).

## Settings

- **Refresh interval** — how often to re-run `petridish menubar` (default 30s,
  matching the xbar plugin's `.30s` convention).
- **Binary path** — explicit path to `petridish`, for installs outside the
  searched directories.

## Uninstall

Right-click the panel → **Applets** → remove it, then:

```sh
rm -rf ~/.local/share/cinnamon/applets/petridish@jkrag
```

## Development

The xbar-text parser is pure JS with no Cinnamon imports (`parser.js`), tested
with node's built-in runner — no npm install needed:

```sh
make cinnamon
```

Debugging inside Cinnamon: `cinnamon-looking-glass` shows applet errors, or
`journalctl --user -f` while reloading the applet. To pick up a code change,
remove and re-add the applet, or reload it from Looking Glass's Extensions tab.

## Other desktops

The applet is Cinnamon-specific. COSMIC, KDE and Waybar users get the same
dropdown from the [SNI tray indicator](../sni-tray/README.md); a GNOME
[Argos](https://github.com/p-e-w/argos) user gets it for free (Argos speaks the
xbar format natively); anything else has `petridish menubar` as a stable
one-command text source to build on.
