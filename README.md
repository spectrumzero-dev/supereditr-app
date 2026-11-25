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

Select text in any Mac app to see suggested fixes. Accept them with one key. SuperEditr uses Apple Intelligence on your Mac, the AI you already pay for (Claude, OpenAI, Gemini), a local model, or nothing at all: a built-in offline checker handles English spelling and grammar on its own.

SuperEditr is built for the Mac first: the shortcuts and settings below are the Mac ones. It also runs on Windows, and on Linux with [Omarchy](https://omarchy.org), with the same engine, styles and panel; each platform has its own page, listed under [Other platforms](#other-platforms), with its install steps, its keys and what differs.

## How it works

The keys in this section are the Mac's; the [Windows](docs/windows.md#shortcuts) and [Linux](docs/linux.md#shortcuts) pages list theirs.

1. **Select text** you're editing in any app. A suggestion panel appears just below it, without taking focus, showing edits as your AI streams its response. With no AI set up, the offline checker handles English spelling and grammar. If your AI fails before returning text, SuperEditr tries Apple Intelligence where available and enabled. If the AI review still fails, SuperEditr reports the failure and offers offline English fixes when the checker can help.
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

Typing or clicking elsewhere hides an automatic suggestion. So does esc, which goes no further: your text stays selected. Automatic suggestions skip a selection longer than 2,000 characters, such as a whole document; the shortcut reviews any length. Change that limit, or turn automatic suggestions off to use the shortcut only, in Settings → General. All shortcuts can be changed in Settings → Shortcuts, and any style can have its own global shortcut.

SuperEditr opens when you log in, so it is running again after you restart your Mac. Turn this off in Settings → General → Open at login.

## Install

### macOS

SuperEditr requires macOS 13 or later. Download `SuperEditr-<version>.dmg` from the [latest release](https://github.com/spectrumzero-dev/supereditr-app/releases/latest), drag SuperEditr to Applications, open it and allow Accessibility access when asked. [SuperEditr on macOS](docs/macos.md#install) has every step, including connecting a model.

### Windows

Windows builds are early access, for Windows 11 on x64. Download `SuperEditr_<version>_x64-setup.exe` from the newest release that has it on [Releases](https://github.com/spectrumzero-dev/supereditr-app/releases) and run it. The installer isn't code-signed, so SmartScreen may warn you, and Smart App Control, where it's turned on, blocks it: [SuperEditr on Windows](docs/windows.md#install) says what to do about SmartScreen and how to check whether Smart App Control is on.

### Linux (Omarchy)

SuperEditr runs on [Omarchy](https://omarchy.org) on x86-64; other Linux desktops aren't supported yet. Download the newest Linux release's `PKGBUILD` and run `makepkg -si`: [SuperEditr on Linux](docs/linux.md#install) has the commands and the setup step, and the [Omarchy bar widget](docs/linux.md#the-omarchy-bar-widget).

## Choosing a model

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

SuperEditr has no accounts or analytics, and its only server is the update server. Official macOS and Windows release builds check for updates through `updates.supereditr.app`, which counts each check by app version, operating system and processor (Apple silicon or Intel on a Mac) and passes it on to `latest.json` on the newest GitHub Release for that system. The app sends no identifier, and the server doesn't store IP addresses. Builds from source and Linux builds never check. The text you review goes from your computer directly to the provider you chose, or nowhere if you use the offline checker or a local model. Password fields are never read, and neither is anything in a password manager: Apple's Passwords app, Keychain Access, 1Password, NordPass, Bitwarden and KeePassXC (and KeePass on Windows, and Seahorse on Linux). Those apps and terminals are also excluded by default (Settings → Privacy), where a password manager can't be removed. Add any other app to the excluded apps there, with **Add app…** on a Mac or by its program file name on Windows. To turn SuperEditr off in the app you're using, click its menu-bar icon (on Windows, its icon in the notification area) and choose Turn off in *that app*; Turn on in *that app* undoes it. The menu doesn't offer either in a password manager. To diagnose a problem in a particular app, turn on Settings → General → Keep a diagnostic log. SuperEditr then keeps a small local log (`~/Library/Logs/SuperEditr/capture.log` on a Mac, `%LOCALAPPDATA%\SuperEditr\Logs\capture.log` on Windows) with the app, timing and length of each read and write, never the text. It's off by default, and turning it off deletes the log.

A selection that holds a secret SuperEditr recognizes is never reviewed: not by the offline checker, and never sent to any AI. It recognizes private keys, access tokens and API keys from common services (GitHub, AWS, OpenAI, Anthropic, Stripe, Slack and others), passwords given in settings (`DB_PASSWORD=…`), passwords in links, and random-looking keys right after words like "token" or "API key". It says what it found and on which line, never the secret itself. Detection works by pattern, so a secret in an unrecognized format, or a bare random string with no label (an AWS secret access key on its own, for example), can still get through; don't select text you wouldn't paste into the AI yourself. The [hand-written rules](crates/core/src/sensitive.rs) cite their format documentation where available. Each [imported rule](crates/core/src/sensitive/gitleaks.rs) cites its Gitleaks rule ID.

## Build from source

Every platform needs Rust (via `rustup`) and Node.js 22+. Each platform's page lists the rest of its requirements and how to build it there: [macOS](docs/macos.md#build-from-source), [Windows](docs/windows.md#build-from-source) and [Linux (Omarchy)](docs/linux.md#build-from-source). On every platform, one script runs the checks:

```bash
./scripts/check.sh
```

`check.sh` runs formatting, Clippy, the Rust tests (including provider wire-format tests against a local mock server), a check that the UI's generated IPC types match the Rust ones, the UI type check, tests and build, and the version and Omarchy widget tests. [Testing](docs/testing.md) describes the test layers and where a new test belongs.

`scripts/count-lines.sh` counts the app's own lines of code, leaving out blank lines, comments, tests, build scripts, config files and dependencies.

For UI work, `npm --prefix ui run dev` serves the overlay and settings pages with sample data in a normal browser.

The `supereditr` CLI runs the same engine from a terminal, which is useful for measuring latency:

```bash
cargo run --release -p supereditr-cli -- check --provider anthropic --bench 5 "This are a test."
```

## Other platforms

This README uses the Mac as its example. Each platform runs the same engine, styles and panel; its page covers installing, shortcuts, permissions or setup, building and what differs from the Mac:

- [macOS](docs/macos.md)
- [Windows](docs/windows.md)
- [Linux (Omarchy)](docs/linux.md), including [the Omarchy bar widget](docs/linux.md#the-omarchy-bar-widget)

## Architecture

| Part | Path | Role |
|---|---|---|
| Engine | `crates/core` | Providers (Apple Intelligence, Anthropic, OpenAI, Gemini, Ollama, OpenAI-compatible, Claude CLI), prompts and styles, word diffing, the Harper offline checker, guardrails, settings, account detection, and the rules every platform applies alike (`policy.rs`), such as which apps are password managers. Platform-independent. |
| CLI | `crates/cli` | Terminal front end for the engine. |
| AI edit eval | `crates/eval` | Runs the engine's AI edits over a fixed corpus, scores them and flags regressions against a baseline. See its [README](crates/eval/README.md). |
| Desktop shell | `src-tauri` | Tauri 2 app: menu-bar item, global shortcuts, overlay panel, settings window, keychain, and the review controller (`review.rs`), which runs each review from capture to replacement. |
| macOS layer | `src-tauri/src/platform/macos` | Accessibility capture and replacement, clipboard transactions, keyboard-layout-aware ⌘C/⌘V, selection watcher. |
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

Another platform needs only its own `src-tauri/src/platform` implementation, as Linux and Windows have, providing what `src-tauri/src/platform/mod.rs` lists; the engine, UI and settings are shared.

## Contributing

Fixes and improvements are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md). Report security issues as described in [SECURITY.md](SECURITY.md).

## License

Copyright 2026 Spectrum Zero LLC. Licensed under the [Apache License 2.0](LICENSE). The licence grants no rights to use the SuperEditr name or logo as a trademark; see [TRADEMARKS.md](TRADEMARKS.md).
