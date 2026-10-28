# SuperEditr

Select text in any Mac app and get fixes in about half a second. Accept them with one key. SuperEditr uses Apple Intelligence on your Mac, the AI you already pay for (Claude, OpenAI, Gemini), a local model, or nothing at all: a built-in offline checker handles English spelling and grammar on its own.

## How it works

1. **Select text** you're editing in any app. A suggestion panel appears just below it, without taking focus, showing instant offline fixes and then the model's version as it streams in.
2. **Press ⌥↩** to apply it, or **⌘E E** (hold ⌘, press E twice) to move into the panel and review with the keyboard:

   | Key | Action |
   |---|---|
   | ↩ | Apply what you see |
   | tab / ⇧tab | Step through individual changes |
   | ␣ | Keep the original for the focused change |
   | A | Always keep a name or term you kept: add it to your words |
   | 1–7 | Switch style: Proofread, Polish, Shorter, Friendlier, Professional, Simplify, To English |
   | / | Type an instruction, e.g. "warmer and shorter" |
   | R | Try again |
   | ⌘C | Copy instead of replacing |
   | esc | Close |

3. **⌃⌥⇧E** fixes the selection in place without showing anything.

### Your words

Add names, places, and terms under Settings → Your words, as a word ("Rhyan") or a phrase ("1234 Glenmoor Ct"). The offline checker treats them as correctly spelled, the AI is told to keep the ones in the selected text (the rest of the list stays on your Mac), and a change that drops one anyway starts out rejected. When you keep your original spelling of an unfamiliar word in the panel, **A** adds it. Even before you add them, the offline checker leaves capitalized names in the middle of a sentence alone ("I am taking Rhyan to tennis").

Typing or clicking elsewhere hides an automatic suggestion. Turn automatic suggestions off in Settings → Behavior to use the shortcut only. All shortcuts can be changed, and any style can have its own global shortcut.

## Install

1. Open `SuperEditr-<version>.dmg` and drag SuperEditr to Applications.
2. Open SuperEditr. Its icon (a pen nib finishing a stroke) appears in the menu bar, and the Get Started page opens.
3. **Allow Accessibility access** when asked: System Settings → Privacy & Security → Accessibility → SuperEditr. SuperEditr only reads text you select and never reads password fields.
4. **Connect a model** (optional): click **Find my AI accounts**, which opens Models and scans. SuperEditr finds Apple Intelligence, API keys exported in your shell profile, an OpenAI key saved by the Codex CLI, a signed-in Claude Code CLI, and Ollama or LM Studio running on your Mac. Click **Use** on one, or paste a key under Models. Keys are stored in your Keychain.
5. Try the sample sentence on the Get Started page, then select text anywhere.

Official signed and notarized builds are on the [Releases page](https://github.com/spectrumzero-dev/supereditr-app/releases) and update themselves. A DMG you build with `scripts/build.sh` is ad-hoc signed: right-click SuperEditr in Applications and choose **Open** the first time.

### Choosing a model

| Option | Speed | Cost | Notes |
|---|---|---|---|
| Apple Intelligence | ~0.4 s to first words, ~0.7 s total (measured on a Mac Studio) | Free | Runs on your Mac; text never leaves it. Needs macOS 26+ on Apple silicon with Apple Intelligence turned on. Its small context suits proofreading and short rewrites |
| Claude API key | Fastest | Under 10 cents per thousand reviews with Haiku 5.5 | Default model `claude-haiku-5-5`, thinking off |
| OpenAI API key | Fast | Low | Default `gpt-6-luna`, lowest reasoning effort the model accepts |
| Gemini API key | Fast | Free tier available from Google AI Studio | Newest Flash-Lite model |
| Ollama (local) | Depends on your Mac | Free | Fully private. Download `gemma3:4b` or `qwen3:4b` from Settings → Models |
| Claude subscription (Claude Code CLI) | 2–6 s | Included in your plan | Runs your signed-in `claude` binary; slow to start |
| None | Instant | Free | English spelling and grammar only |

Consumer Claude, ChatGPT and Gemini plans don't give other apps API access. The Claude Code bridge is the supported way to use a subscription; for everyday speed, an API key is better.

## Privacy

SuperEditr has no servers, accounts, or analytics. The text you review goes from your Mac directly to the provider you chose, or nowhere if you use the offline checker or a local model. Password fields are never read. Apple's Passwords app, the password managers 1Password, NordPass, Bitwarden and KeePassXC, Keychain Access, and terminals are excluded by default (Settings → Behavior); add any other app there. To help diagnose capture problems, SuperEditr keeps a small local log, `~/Library/Logs/SuperEditr/capture.log`, with the app, timing and length of each read and write, never the text.

A selection that holds a secret SuperEditr recognizes is never reviewed: not by the offline checker, and never sent to any AI. It recognizes private keys, access tokens and API keys from common services (GitHub, AWS, OpenAI, Anthropic, Stripe, Slack and others), passwords given in settings (`DB_PASSWORD=…`), passwords in links, and random-looking keys right after words like "token" or "API key". It says what it found and on which line, never the secret itself. Detection works by pattern, so a secret in an unrecognized format, or a bare random string with no label (an AWS secret access key on its own, for example), can still get through; don't select text you wouldn't paste into the AI yourself. The rules are listed in [`crates/core/src/sensitive.rs`](crates/core/src/sensitive.rs), each naming the document that defines its format where there is one.

## Build from source

Requirements: macOS 13+, Xcode command-line tools, Rust (stable, via `rustup`), Node.js 22+.

```bash
./scripts/check.sh
```

```bash
./scripts/build.sh
```

`check.sh` runs formatting, Clippy, the Rust tests (including provider wire-format tests against a local mock server), the UI type check and UI tests. `build.sh` writes `build/SuperEditr.app` and `build/SuperEditr-<version>.dmg`. Set `SUPEREDITR_SIGNING_IDENTITY` to an Apple Development identity so macOS keeps the Accessibility permission across rebuilds; without it the app is ad-hoc signed.

For UI work, `npm --prefix ui run dev` serves the overlay and settings pages with sample data in a normal browser.

The `supereditr` CLI runs the same engine from a terminal, which is useful for measuring latency:

```bash
cargo run --release -p supereditr-cli -- check --provider anthropic --bench 5 "This are a test."
```

## Architecture

| Part | Path | Role |
|---|---|---|
| Engine | `crates/core` | Providers (Apple Intelligence, Anthropic, OpenAI, Gemini, Ollama, OpenAI-compatible, Claude CLI), prompts and styles, word diffing, the Harper offline checker, guardrails, settings, account detection. Platform-independent. |
| CLI | `crates/cli` | Terminal front end for the engine. |
| Desktop shell | `src-tauri` | Tauri 2 app: menu-bar item, global shortcuts, overlay panel, settings window, keychain. |
| macOS layer | `src-tauri/src/platform/macos.rs` | Accessibility capture and replacement, clipboard transactions, keyboard-layout-aware ⌘C/⌘V, selection watcher. |
| UI | `ui` | Svelte 5 overlay (12 KB) and settings pages. |
| Apple Intelligence helper | `helpers/foundation-models` | 96 KB Swift program bundled in the app. FoundationModels is Swift-only, so the engine keeps this helper running and talks to it over JSON lines. |

A review runs like this:

1. A selection ends (or the shortcut is pressed).
2. SuperEditr reads it through Accessibility off the UI thread, with a 300 ms timeout per call, and warms the provider connection at the same time.
3. The overlay appears at the selection with offline fixes (~20 ms).
4. The model streams the edited text; SuperEditr diffs it word by word into individual changes.
5. On ↩, SuperEditr confirms the same field and selection are still there, then writes the text back: an Accessibility write for native fields, a clipboard-preserving paste for web and Electron apps. ⌘Z in the app undoes it.

Another platform needs only its own `src-tauri/src/platform` implementation (on Windows, with UI Automation); the engine, UI and settings are shared.

## Status

This is a first working version.

- **Verified:**
  - Every automated check above passes.
  - The release app builds (13.5 MB, DMG 6.7 MB), launches, and stays running.
  - The offline checker fixes errors in about 20 ms.
  - The Claude Code subscription bridge proofreads end to end.
  - Apple Intelligence proofreads end to end on a Mac Studio: median 416 ms to first words and 708 ms total over four warm runs, with every error in the test sentence fixed.
  - The overlay and settings pages render correctly in light and dark mode.
  - Live UI tests (`tests/ui/smoke.swift`, run by `scripts/ui-smoke.sh` on a Mac with Accessibility access) cover automatic suggestions in TextEdit, waiting for a Shift selection to end, opening the keyboard review, ignoring terminal output, shortcut recording, adding a kept name to Your words, and refusing a selection that holds a token.
- **Not yet covered by automated tests:** applying a revision, capture in other apps (Notes, Mail, Chrome/Gmail, Slack, Claude, ChatGPT), and real-world latency with an API key.

## Contributing

Fixes and improvements are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md). Report security issues as described in [SECURITY.md](SECURITY.md).

## License

Copyright 2026 Spectrum Zero LLC. Licensed under the [Apache License 2.0](LICENSE). The licence grants no rights to use the SuperEditr name or logo as a trademark; see [TRADEMARKS.md](TRADEMARKS.md).
