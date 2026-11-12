<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" width="128" height="128" alt="SuperEditr icon: a blue pen nib finishing a stroke on a yellow square">
</p>

<h1 align="center">SuperEditr</h1>

<p align="center">Fast proofreading in any Mac app. Also runs on Windows, and on Linux with Omarchy.</p>

<p align="center">
  <a href="https://github.com/spectrumzero-dev/supereditr-app/actions/workflows/ci.yml"><img src="https://github.com/spectrumzero-dev/supereditr-app/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI status"></a>
  <a href="https://github.com/spectrumzero-dev/supereditr-app/releases/latest"><img src="https://img.shields.io/github/v/release/spectrumzero-dev/supereditr-app" alt="Latest release"></a>
  <img src="https://img.shields.io/badge/macOS-13%2B-blue" alt="Requires macOS 13 or later">
  <img src="https://img.shields.io/badge/Windows-11-blue" alt="Runs on Windows 11">
  <img src="https://img.shields.io/badge/Linux-Omarchy-blue" alt="Runs on Linux with Omarchy">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache%202.0-blue" alt="Apache 2.0 license"></a>
</p>

<p align="center"><a href="https://supereditr.app">supereditr.app</a> · <a href="https://github.com/spectrumzero-dev/supereditr-app/releases">Releases</a> · <a href="#build-from-source">Build from source</a> · <a href="#other-platforms">Other platforms</a></p>

Select text in any Mac app and get fixes in about half a second. Accept them with one key. SuperEditr uses Apple Intelligence on your Mac, the AI you already pay for (Claude, OpenAI, Gemini), a local model, or nothing at all: a built-in offline checker handles English spelling and grammar on its own.

SuperEditr is built for the Mac first: the shortcuts, settings and install steps below are the Mac ones. It also runs on Windows, and on Linux with [Omarchy](https://omarchy.org), with the same engine, styles and panel; [Other platforms](#other-platforms) covers installing it there and what differs.

## How it works

1. **Select text** you're editing in any app. A suggestion panel appears just below it, without taking focus, showing instant offline fixes and then the model's version as it streams in.
2. **Press ⌥↩︎** to apply it, or **⌘E E** (hold ⌘, press E twice) to move into the panel and review with the keyboard:

   | Key | Action |
   |---|---|
   | ↩︎ | Apply what you see |
   | tab / ⇧tab | Step through individual changes |
   | ␣ | Keep the original for the focused change |
   | A | Always keep a name or term you kept: add it to your dictionary |
   | 1–7 | Switch style: Proofread, Polish, Shorter, Friendlier, Professional, Simplify, To English |
   | / | Type an instruction, e.g. "warmer and shorter" |
   | R | Try again |
   | ⌘C | Copy instead of replacing |
   | esc | Close |

   ⌥↩︎ and ↩︎ only ever replace. In text SuperEditr can't replace, such as a web page, the panel says so and ⌘C copies the fix.

3. **⌃⌥⇧E** fixes the selection in place without showing anything.

### Your dictionary

Add names, places, and terms under Settings → Dictionary, as a word ("Rhyan") or a phrase ("1234 Glenmoor Ct"). The offline checker treats them as correctly spelled, the AI is told to keep the ones in the selected text (the rest of the list stays on your Mac), and a change that drops one anyway starts out rejected. When you keep your original spelling of an unfamiliar word in the panel, **A** adds it. Even before you add them, the offline checker leaves capitalized names in the middle of a sentence alone ("I am taking Rhyan to tennis"). Settings → Dictionary also sets English spelling (American, British, Canadian, Australian or Indian), which the offline checker and the AI both follow.

With nothing selected, ⌘E E and ⌃⌥⇧E select the sentence before the cursor and review that. In an app that hides its text from Accessibility, select the text yourself. Turn this off in Settings → Shortcuts.

Applying a proofread changes only the words it fixes, so code, bold and links in the rest of the text stay as they are. Undo takes one step per fix. A different style, or an app that doesn't allow it, replaces the whole selection instead. Turn this off in Settings → General → Keep formatting when applying fixes.

Typing or clicking elsewhere hides an automatic suggestion. Turn automatic suggestions off in Settings → General to use the shortcut only. All shortcuts can be changed in Settings → Shortcuts, and any style can have its own global shortcut.

SuperEditr opens when you log in, so it is running again after you restart your Mac. Turn this off in Settings → General → Open at login.

## Install

On another platform, see [Other platforms](#other-platforms).

On a Mac, SuperEditr requires macOS 13 or later. Apple Intelligence needs macOS 26 or later on Apple silicon.

1. Open `SuperEditr-<version>.dmg` and drag SuperEditr to Applications.
2. Open SuperEditr. Its icon (a pen nib finishing a stroke) appears in the menu bar, and the Get Started page opens.
3. **Allow Accessibility access** when asked: System Settings → Privacy & Security → Accessibility → SuperEditr. SuperEditr only reads text you select and never reads password fields.
4. **Connect a model** (optional): click **Find my AI accounts**, which opens Models and scans. SuperEditr finds Apple Intelligence, API keys exported in your shell profile, an OpenAI key saved by the Codex CLI, a signed-in Claude Code CLI or the copy of Claude Code that comes with the Claude desktop app, and Ollama or LM Studio running on your Mac. Click **Use** on one, or paste a key under Models. Keys are stored in your Keychain.
5. Try the sample sentence on the Get Started page, then select text anywhere.

Signed, notarized builds are published on the [Releases page](https://github.com/spectrumzero-dev/supereditr-app/releases) and update themselves. A DMG you build with `scripts/build.sh` is ad-hoc signed: right-click SuperEditr in Applications and choose **Open** the first time.

### Choosing a model

| Option | Speed | Cost | Notes |
|---|---|---|---|
| Apple Intelligence | Under a second | Free | Runs on your Mac; text never leaves it. Needs macOS 26+ on Apple silicon with Apple Intelligence turned on. Its small context suits proofreading and short rewrites |
| Claude API key | Fastest | A small fraction of a cent per review | Defaults to Anthropic's smallest, fastest model with thinking off |
| OpenAI API key | Fast | Low | Defaults to a small model at the lowest reasoning effort it accepts |
| Gemini API key | Fast | Free tier available from Google AI Studio | Defaults to a Flash-Lite model |
| Ollama or LM Studio (local) | Depends on your Mac | Free | Fully private. Settings → Models can download a small model for Ollama |
| Claude subscription (Claude Code CLI) | A few seconds | Included in your plan | Runs your signed-in `claude` binary; slow to start |
| None | Instant | Free | English spelling and grammar only |

Settings → Models shows the exact model each option uses and lets you pick another.

With Apple Intelligence, Ollama, LM Studio or another OpenAI-compatible server, the offline checker also goes over the AI's answer for mistakes small models can miss ("should of"), and the panel marks its fixes as the rule-based checker's. Answers from Claude, OpenAI and Gemini are used as they are.

Consumer Claude, ChatGPT and Gemini plans don't give other apps API access. The Claude Code bridge is the supported way to use a subscription; for everyday speed, an API key is better.

## Privacy

SuperEditr has no accounts or analytics, and its only server is the update server. Official macOS release builds check for updates through `updates.supereditr.app`, which counts each check by app version and Mac architecture (Apple silicon or Intel) and passes it on to `latest.json` on the newest GitHub Release. The app sends no identifier, and the server doesn't store IP addresses. Builds from source, Windows builds and Linux builds never check. The text you review goes from your computer directly to the provider you chose, or nowhere if you use the offline checker or a local model. Password fields are never read. Apple's Passwords app, the password managers 1Password, NordPass, Bitwarden and KeePassXC (and KeePass on Windows), Keychain Access, and terminals are excluded by default (Settings → Privacy); add any other app there, with **Add app…** on a Mac or by its program file name on Windows. To turn SuperEditr off in the app you're using, click its menu-bar icon (on Windows, its icon in the notification area) and choose Turn off in *that app*; Turn on in *that app* undoes it. To diagnose a problem in a particular app, turn on Settings → General → Keep a diagnostic log. SuperEditr then keeps a small local log (`~/Library/Logs/SuperEditr/capture.log` on a Mac, `%LOCALAPPDATA%\SuperEditr\Logs\capture.log` on Windows) with the app, timing and length of each read and write, never the text. It's off by default, and turning it off deletes the log.

A selection that holds a secret SuperEditr recognizes is never reviewed: not by the offline checker, and never sent to any AI. It recognizes private keys, access tokens and API keys from common services (GitHub, AWS, OpenAI, Anthropic, Stripe, Slack and others), passwords given in settings (`DB_PASSWORD=…`), passwords in links, and random-looking keys right after words like "token" or "API key". It says what it found and on which line, never the secret itself. Detection works by pattern, so a secret in an unrecognized format, or a bare random string with no label (an AWS secret access key on its own, for example), can still get through; don't select text you wouldn't paste into the AI yourself. The rules are listed in [`crates/core/src/sensitive.rs`](crates/core/src/sensitive.rs), each naming the document that defines its format where there is one.

## Build from source

On a Mac, the requirements are macOS 13+, Xcode command-line tools, Rust (stable, via `rustup`) and Node.js 22+. To build on another platform, see [Other platforms](#other-platforms).

```bash
./scripts/check.sh
```

```bash
./scripts/build.sh
```

`check.sh` runs formatting, Clippy, the Rust tests (including provider wire-format tests against a local mock server), the UI type check and UI tests. `build.sh` writes `build/SuperEditr.app` and `build/SuperEditr-<version>.dmg`. Set `SUPEREDITR_SIGNING_IDENTITY` to an Apple Development identity so macOS keeps the Accessibility permission across rebuilds; without it the app is ad-hoc signed.

`scripts/ui-smoke.sh` runs the live UI tests in `tests/ui/smoke.swift` against `build/SuperEditr.app` and saves screenshots to `build/ui-smoke`. It needs an unlocked display and Accessibility access, and it types into real apps such as TextEdit, so don't use the Mac while it runs. Where it can't run, it warns and skips.

`scripts/count-lines.sh` counts the app's own lines of code, leaving out blank lines, comments, tests, build scripts, config files and dependencies.

For UI work, `npm --prefix ui run dev` serves the overlay and settings pages with sample data in a normal browser.

The `supereditr` CLI runs the same engine from a terminal, which is useful for measuring latency:

```bash
cargo run --release -p supereditr-cli -- check --provider anthropic --bench 5 "This are a test."
```

## Other platforms

Everything above describes the Mac app. Each platform below runs the same engine, styles and panel; its section covers installing, building and what differs from the Mac.

### Linux (Omarchy)

SuperEditr also runs on [Omarchy](https://omarchy.org), the Arch Linux setup built on the Hyprland compositor, on x86-64. Other Linux desktops aren't supported yet: SuperEditr starts there, but its shortcuts and panel need Hyprland. Every feature is free on Linux.

To install the newest Linux release, download its `PKGBUILD` and run `makepkg`, which checks the download against its SHA-256 and installs the `supereditr-bin` package:

```bash
mkdir -p supereditr && cd supereditr
curl -fLO "https://raw.githubusercontent.com/spectrumzero-dev/supereditr-app/main/packaging/arch/{PKGBUILD,supereditr.install}"
makepkg -si
```

Run the same commands again to install a new version. Then open SuperEditr and run Set up Omarchy, which lists each change before it makes it.

#### The Omarchy bar widget

The SuperEditr bar widget shows whether SuperEditr is running and which shortcuts it uses. Click it to review or quick-fix the selection, start SuperEditr or open Settings, or right-click it to open Settings. Install SuperEditr first, then add the widget:

```bash
omarchy plugin add https://github.com/spectrumzero-dev/supereditr-app --enable
```

SuperEditr's own icon stays in the system tray. To keep only the widget, right-click the tray's arrow and choose Hide beside SuperEditr. To remove the widget:

```bash
omarchy plugin remove app.supereditr
```

The widget runs inside Omarchy's shell with your permissions. It reads `status.json` and `settings.json` in `~/.config/app.supereditr`, checks for a running `supereditr` process, and runs the `supereditr` command, which hands its action to the running app; it changes no settings and installs nothing else. Its files are `manifest.json` and `packaging/omarchy/`.

To build it from source instead, the requirements are Arch Linux on x86-64, Rust (via `rustup`), Node.js 22+, and the packages `base-devel webkit2gtk-4.1 gtk3 libayatana-appindicator at-spi2-core librsvg`.

```bash
./scripts/linux-release.sh
cd build/arch && makepkg -si
```

`linux-release.sh` builds `build/supereditr-<version>-x86_64.tar.gz`, and puts it in `build/arch/` beside a copy of `packaging/arch/PKGBUILD` with its version and SHA-256, which installs it as the `supereditr-bin` package. `check.sh` runs the same checks as on macOS.

How it differs from macOS:

- The review shortcut is Super+Alt+E. On Hyprland, SuperEditr adds its binds and the panel's window rule while it runs and never edits `~/.config/hypr/`.
- SuperEditr reads text through the AT-SPI accessibility bus. Where an app exposes no text, such as a terminal, it copies the selection and puts your clipboard back afterwards; it can't replace text there, so Ctrl+C in the panel copies the fix.
- Alt+Enter replaces your text, and Ctrl+C copies the fix.
- Settings → Set up Omarchy lists what it will change before it does: it adds `--force-renderer-accessibility` to the Chromium, Electron and VS Code flag files in `~/.config`, so web pages and Electron apps expose their text, and turns on opening at login. `supereditr uninstall-omarchy` takes out exactly what it added.
- API keys are saved in the Secret Service keyring (GNOME Keyring on Omarchy), not in SuperEditr's settings. Omarchy's default keyring has no password, so any program you run can read them; Settings → Desktop setup says so, and Seahorse (Passwords and Keys) can set one.
- There are no in-app updates: the package manager installs new versions, and SuperEditr never contacts the update server.
- The diagnostic log is `~/.local/state/supereditr/capture.log`.

`scripts/ui-smoke-linux.sh` runs the live Linux UI tests against SuperEditr in a desktop virtual machine on a Proxmox host, pressing real keys through the host; see the script's header for its options.

### Windows

SuperEditr runs on Windows 11 on x64 PCs.

To build it from source, the requirements are Windows 11 on x64, Visual Studio 2022 Build Tools with "Desktop development with C++" (MSVC and the Windows 11 SDK), Rust (via `rustup`), Node.js 22+ and Git for Windows. In Git Bash:

```bash
./scripts/check.sh
./ui/node_modules/.bin/tauri build --bundles nsis
```

The installer is written to `target/release/bundle/nsis/`.

How it differs from macOS:

- The review shortcut is Ctrl+Shift+F2, the same key on common keyboard layouts; quick fix starts unbound. Settings → Shortcuts records others by the key Windows reports, so they follow your keyboard layout. Double presses aren't available, and a review starts only from the shortcut: suggestions don't appear on their own.
- The panel takes the keyboard for the review and gives it back to your app when it closes. Alt+Enter (or Enter) replaces your text, Tab steps through changes, Ctrl+C copies the fix and Esc closes it.
- SuperEditr reads text through UI Automation and writes a fix with a checked paste: just before pasting it checks that the same field and selection are still there, afterwards it checks the text changed as expected, and it puts your clipboard back. When it can't confirm a step, nothing is written, or the panel stays open with the fix to copy. Terminals are copy-only; so are remote-desktop and virtual-machine windows, whose paste goes to another computer, and LibreOffice. Password fields and apps running as administrator are refused. VS Code exposes its editor's text only with `editor.accessibilitySupport` set to `on`.
- Windows needs no permission for SuperEditr to read the text you select.
- API keys are saved in Windows Credential Manager, not in SuperEditr's settings.
- Opening at login is a value under your user's Run key, which Task Manager's Startup apps can also turn off.
- There are no in-app updates: build a newer version and run its installer.
- The diagnostic log is `%LOCALAPPDATA%\SuperEditr\Logs\capture.log`.

## Architecture

| Part | Path | Role |
|---|---|---|
| Engine | `crates/core` | Providers (Apple Intelligence, Anthropic, OpenAI, Gemini, Ollama, OpenAI-compatible, Claude CLI), prompts and styles, word diffing, the Harper offline checker, guardrails, settings, account detection. Platform-independent. |
| CLI | `crates/cli` | Terminal front end for the engine. |
| Desktop shell | `src-tauri` | Tauri 2 app: menu-bar item, global shortcuts, overlay panel, settings window, keychain. |
| macOS layer | `src-tauri/src/platform/macos.rs` | Accessibility capture and replacement, clipboard transactions, keyboard-layout-aware ⌘C/⌘V, selection watcher. |
| Linux layer | `src-tauri/src/platform/linux` | AT-SPI capture and replacement, clipboard transactions, the Hyprland and portal shortcuts and panel placement, Set up Omarchy. |
| Windows layer | `src-tauri/src/platform/windows` | UI Automation capture, the checked paste and its clipboard transaction, `RegisterHotKey` shortcuts by virtual key, the non-activating panel and its placement in physical pixels, open at login. |
| UI | `ui` | Svelte 5 overlay and settings pages. |
| Apple Intelligence helper | `helpers/foundation-models` | Small Swift program bundled in the app. FoundationModels is Swift-only, so the engine keeps this helper running and talks to it over JSON lines. |

A review runs like this:

1. A selection ends (or the shortcut is pressed).
2. SuperEditr reads it through Accessibility off the UI thread, with a 300 ms timeout per call, and warms the provider connection at the same time.
3. The overlay appears at the selection with offline fixes, in milliseconds.
4. The model streams the edited text; SuperEditr diffs it word by word into individual changes.
5. On ↩︎, SuperEditr confirms the same field and selection are still there, then writes the text back: an Accessibility write for native fields, a clipboard-preserving paste for web and Electron apps. ⌘Z in the app undoes it.

Another platform needs only its own `src-tauri/src/platform` implementation, as Linux and Windows have; the engine, UI and settings are shared.

## Contributing

Fixes and improvements are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md). Report security issues as described in [SECURITY.md](SECURITY.md).

## License

Copyright 2026 Spectrum Zero LLC. Licensed under the [Apache License 2.0](LICENSE). The licence grants no rights to use the SuperEditr name or logo as a trademark; see [TRADEMARKS.md](TRADEMARKS.md).
