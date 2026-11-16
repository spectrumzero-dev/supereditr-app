# SuperEditr on Windows

The [README](../README.md) describes SuperEditr with the Mac's keys and settings. This page covers what is particular to Windows: its requirements, installing it, its shortcuts and permissions, how it differs from macOS, building it from source and uninstalling it.

## Requirements

SuperEditr runs on Windows 11 on x64 PCs.

## Install

Windows builds are early access. To install one, download `SuperEditr_<version>_x64-setup.exe` from the newest release that has it on [Releases](https://github.com/spectrumzero-dev/supereditr-app/releases) and run it. It installs for your user only, in `%LOCALAPPDATA%\SuperEditr`, without an administrator prompt, and installs Microsoft Edge WebView2 first if it's missing.

The installer isn't code-signed, so SmartScreen may show "Windows protected your PC": choose More info, then Run anyway. Smart App Control, where it's turned on, blocks unsigned apps and has no exception for a single app, so SuperEditr won't install there; Windows Security › App & browser control › Smart App Control settings shows whether it's on.

## Shortcuts

- The review shortcut is Ctrl+Shift+F2, the same key on common keyboard layouts; quick fix starts unbound. Settings → Shortcuts records others by the key Windows reports, so they follow your keyboard layout. Double presses aren't available, and a review starts only from the shortcut: suggestions don't appear on their own.
- The panel takes the keyboard for the review and gives it back to your app when it closes. Alt+Enter (or Enter) replaces your text, Tab steps through changes, Ctrl+C copies the fix and Esc closes it.

## Permissions

Windows needs no permission for SuperEditr to read the text you select.

## How it differs from macOS

The shortcuts and permissions above differ from the Mac's too. Beyond them:

- SuperEditr reads text through UI Automation and writes a fix with a checked paste: just before pasting it checks that the same field and selection are still there, afterwards it checks the text changed as expected, and it puts your clipboard back. When it can't confirm a step, nothing is written, or the panel stays open with the fix to copy. Terminals are copy-only; so are remote-desktop and virtual-machine windows, whose paste goes to another computer, and LibreOffice. Password fields and apps running as administrator are refused. VS Code exposes its editor's text only with `editor.accessibilitySupport` set to `on`.
- API keys are saved in Windows Credential Manager, not in SuperEditr's settings.
- Opening at login is a value under your user's Run key, which Task Manager's Startup apps can also turn off.
- Release builds download an update when they find one and install it when you're not using SuperEditr (no review, panel or SuperEditr window open, and no typing for a minute), when you choose Restart to Update, or when you quit. The installer starts SuperEditr again afterwards.
- The diagnostic log is `%LOCALAPPDATA%\SuperEditr\Logs\capture.log`.

## Build from source

To build it from source, the requirements are Windows 11 on x64, Visual Studio 2022 Build Tools with "Desktop development with C++" (MSVC and the Windows 11 SDK), Rust (via `rustup`), Node.js 22+ and Git for Windows. In Git Bash:

```bash
./scripts/check.sh
./ui/node_modules/.bin/tauri build --bundles nsis
```

The installer is written to `target/release/bundle/nsis/`.

`scripts/ui-smoke-windows.sh` runs the live Windows UI tests on a disposable Windows test machine over SSH; see the script's header for its options.

## Uninstall

To remove SuperEditr, use Settings › Apps › Installed apps; tick "Delete the application data" to also delete its settings, its log and the API keys it saved.
