# SuperEditr for the Omarchy bar

A bar widget for [Omarchy](https://omarchy.org) that shows whether [SuperEditr](https://supereditr.app) is running and which shortcuts it uses. Click it to review or quick-fix the selection, start SuperEditr or open its Settings, or right-click it to open Settings.

This repository is published automatically from the `packaging/omarchy` folder of [spectrumzero-dev/supereditr-app](https://github.com/spectrumzero-dev/supereditr-app). Send issues and pull requests there; this repository accepts no direct changes.

## Install

The widget needs SuperEditr itself, which [supereditr.app](https://supereditr.app/#omarchy) explains how to install. Then add the widget:

```bash
omarchy plugin add https://github.com/spectrumzero-dev/supereditr-omarchy --enable
```

SuperEditr's own icon stays in the system tray. To keep only the widget, right-click the tray's arrow and choose Hide beside SuperEditr.

To update the widget, or remove it:

```bash
omarchy plugin update app.supereditr
```

```bash
omarchy plugin remove app.supereditr
```

## What it does

The widget runs inside Omarchy's shell with your permissions. It reads `status.json` and `settings.json` in `~/.config/app.supereditr`, checks for a running `supereditr` process, and runs the `supereditr` command, which hands its action to the running app. It changes no settings and installs nothing else.

## License

Apache-2.0, like SuperEditr.
