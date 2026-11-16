# SuperEditr on macOS

The [README](../README.md) describes SuperEditr with the Mac's keys and settings. This page covers what is particular to the Mac: its requirements, installing it (including the Accessibility permission), its shortcuts and building it from source. The [Windows](windows.md#how-it-differs-from-macos) and [Linux](linux.md#how-it-differs-from-macos) pages list how those platforms differ from the Mac.

## Requirements

On a Mac, SuperEditr requires macOS 13 or later. Apple Intelligence needs macOS 26 or later on Apple silicon.

## Install

1. Open `SuperEditr-<version>.dmg` and drag SuperEditr to Applications.
2. Open SuperEditr. Its icon (a pen nib finishing a stroke) appears in the menu bar, and the Get Started page opens.
3. **Allow Accessibility access** when asked: System Settings → Privacy & Security → Accessibility → SuperEditr. SuperEditr only reads text you select and never reads password fields.
4. **Connect a model** (optional): click **Find my AI accounts**, which opens Models and scans. SuperEditr finds Apple Intelligence, API keys exported in your shell profile, an OpenAI key saved by the Codex CLI, a signed-in Claude Code CLI or the copy of Claude Code that comes with the Claude desktop app, and Ollama or LM Studio running on your Mac. Click **Use** on one, or paste a key under Models. Keys are stored in your Keychain.
5. Try the sample sentence on the Get Started page, then select text anywhere.

Signed, notarized builds are published on the [Releases page](https://github.com/spectrumzero-dev/supereditr-app/releases) and update themselves. A DMG you build with `scripts/build.sh` is ad-hoc signed: right-click SuperEditr in Applications and choose **Open** the first time.

## Shortcuts

The keys in the README's [How it works](../README.md#how-it-works) are the Mac's.

## Build from source

On a Mac, the requirements are macOS 13+, Xcode command-line tools, Rust (stable, via `rustup`) and Node.js 22+. Run the checks as the README's [Build from source](../README.md#build-from-source) describes, then build the app:

```bash
./scripts/build.sh
```

`build.sh` writes `build/SuperEditr.app` and `build/SuperEditr-<version>.dmg`. Set `SUPEREDITR_SIGNING_IDENTITY` to an Apple Development identity so macOS keeps the Accessibility permission across rebuilds; without it the app is ad-hoc signed.

`scripts/ui-smoke.sh` runs the live UI tests in `tests/ui/smoke.swift` against `build/SuperEditr.app` and saves screenshots to `build/ui-smoke`. It needs an unlocked display and Accessibility access, and it types into real apps such as TextEdit, so don't use the Mac while it runs. Where it can't run, it warns and skips.
