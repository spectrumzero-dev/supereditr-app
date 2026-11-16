# SuperEditr on Linux (Omarchy)

The [README](../README.md) describes SuperEditr with the Mac's keys and settings. This page covers what is particular to Linux: its requirements, installing it, its shortcuts and setup, how it differs from macOS, building it from source, and the Omarchy bar widget.

## Requirements

SuperEditr also runs on [Omarchy](https://omarchy.org), the Arch Linux setup built on the Hyprland compositor, on x86-64. Other Linux desktops aren't supported yet: SuperEditr starts there, but its shortcuts and panel need Hyprland. Every feature is free on Linux.

## Install

To install the newest Linux release, download its `PKGBUILD` and run `makepkg`, which checks the download against its SHA-256 and installs the `supereditr-bin` package:

```bash
mkdir -p supereditr && cd supereditr
curl -fLO "https://raw.githubusercontent.com/spectrumzero-dev/supereditr-app/main/packaging/arch/{PKGBUILD,supereditr.install}"
makepkg -si
```

Run the same commands again to install a new version. Then open SuperEditr and run Set up Omarchy, which lists each change before it makes it.

## Shortcuts

- The review shortcut is Super+Alt+E. On Hyprland, SuperEditr adds its binds and the panel's window rule while it runs and never edits `~/.config/hypr/`.
- Alt+Enter replaces your text, and Ctrl+C copies the fix.

## Setup

Settings → Set up Omarchy lists what it will change before it does: it adds `--force-renderer-accessibility` to the Chromium, Electron and VS Code flag files in `~/.config`, so web pages and Electron apps expose their text, and turns on opening at login. `supereditr uninstall-omarchy` takes out exactly what it added.

## How it differs from macOS

The shortcuts and setup above differ from the Mac's too. Beyond them:

- SuperEditr reads text through the AT-SPI accessibility bus. Where an app exposes no text, such as a terminal, it copies the selection and puts your clipboard back afterwards; it can't replace text there, so Ctrl+C in the panel copies the fix.
- API keys are saved in the Secret Service keyring (GNOME Keyring on Omarchy), not in SuperEditr's settings. Omarchy's default keyring has no password, so any program you run can read them; Settings → Desktop setup says so, and Seahorse (Passwords and Keys) can set one.
- There are no in-app updates: the package manager installs new versions, and SuperEditr never contacts the update server.
- The diagnostic log is `~/.local/state/supereditr/capture.log`.

## Build from source

To build it from source instead, the requirements are Arch Linux on x86-64, Rust (via `rustup`), Node.js 22+, and the packages `base-devel webkit2gtk-4.1 gtk3 libayatana-appindicator at-spi2-core librsvg`.

```bash
./scripts/linux-release.sh
cd build/arch && makepkg -si
```

`linux-release.sh` builds `build/supereditr-<version>-x86_64.tar.gz`, and puts it in `build/arch/` beside a copy of `packaging/arch/PKGBUILD` with its version and SHA-256, which installs it as the `supereditr-bin` package. `check.sh` runs the same checks as on macOS.

`scripts/ui-smoke-linux.sh` runs the live Linux UI tests against SuperEditr in a desktop virtual machine on a Proxmox host, pressing real keys through the host; see the script's header for its options.

## The Omarchy bar widget

The SuperEditr bar widget shows whether SuperEditr is running and which shortcuts it uses. Click it to review or quick-fix the selection, start SuperEditr or open Settings, or right-click it to open Settings. Install SuperEditr first, then add the widget:

```bash
omarchy plugin add https://github.com/spectrumzero-dev/supereditr-omarchy --enable
```

SuperEditr's own icon stays in the system tray. To keep only the widget, right-click the tray's arrow and choose Hide beside SuperEditr. To remove the widget:

```bash
omarchy plugin remove app.supereditr
```

The widget runs inside Omarchy's shell with your permissions. It reads `status.json` and `settings.json` in `~/.config/app.supereditr`, checks for a running `supereditr` process, and runs the `supereditr` command, which hands its action to the running app; it changes no settings and installs nothing else. Its source is `packaging/omarchy/`; each release that ships Linux publishes it to [supereditr-omarchy](https://github.com/spectrumzero-dev/supereditr-omarchy), the repository Omarchy installs it from.
