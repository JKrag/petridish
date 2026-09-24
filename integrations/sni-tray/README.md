# Tray indicator (StatusNotifierItem — COSMIC, KDE, Waybar, …)

The desktop-agnostic sibling of the [xbar plugin](../xbar/README.md) and the
[Cinnamon applet](../cinnamon/README.md): a petri-dish icon in any
StatusNotifierItem tray host, `working/total` in the tooltip (SNI has no text
label next to the icon), and the full dropdown — live sessions on top, buckets
below, click opens the project directory, plus a "Rescan now" item — as a
dbusmenu.

Like the other frontends it is a thin presenter over one command:

```sh
petridish menubar
```

re-run whenever `~/.petridish/projects.json` changes (file monitor), when the
menu opens, and on a 60-second fallback timer. All content decisions stay in
`menubar.rs`, shared with every other frontend.

Tested on COSMIC (Pop!_OS 24.04). Should work on anything that hosts SNI items
with dbusmenu: KDE Plasma, Waybar's `tray` module, swaybar via a helper, etc.
GNOME needs an AppIndicator extension. Cinnamon users should prefer the
[native applet](../cinnamon/README.md).

## Requirements

Python 3 with PyGObject (`gi`: GLib/Gio only — no GTK). Preinstalled on most
GTK-shipping distros; otherwise `python3-gi` (Debian/Ubuntu) or `python-gobject`
(Arch).

## Install

```sh
cp integrations/sni-tray/petridish-tray ~/.local/bin/
mkdir -p ~/.local/share/icons/hicolor/64x64/apps
cp integrations/sni-tray/petridish.png ~/.local/share/icons/hicolor/64x64/apps/
```

Run it once in a terminal to check it appears, then autostart it. With
systemd:

```sh
cp integrations/sni-tray/petridish-tray.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now petridish-tray.service
```

(Waybar/sway users may prefer `exec ~/.local/bin/petridish-tray` in their
compositor config instead.)

The script finds the `petridish` binary on `PATH`, `~/.cargo/bin`, or
`~/.local/bin` — the usual not-launched-from-a-shell problem.

## SNI host quirks encoded in the script

Learned on COSMIC, harmless elsewhere:

- An item that does not export a `com.canonical.dbusmenu` is not drawn at all.
- Unregistering an item leaves a ghost icon, so the item stays registered for
  its lifetime and degrades its menu contents on errors instead — the same
  "always print something" contract the xbar plugin has.

## Development

The xbar-text parser is dependency-free and mirrors the Cinnamon applet's
`parser.js` (same fixtures, same expectations — the two must agree on
menubar.rs's output):

```sh
make sni-tray
```
