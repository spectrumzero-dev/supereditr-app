# AGENTS.md

SuperEditr: a Rust/Tauri proofreading app for macOS, Windows and Linux. `README.md` describes the layout and how to build.

## Checks

Run `./scripts/check.sh` before every commit. A commit that changes only Markdown files can skip it: the script checks no Markdown, and CI still runs it on every pull request.

Put each test at the lowest layer that can catch the bug. Before adding a test or a live UI (smoke) check, read `docs/testing.md`: the layers, what each one proves and the command that runs it.

## Where code goes

- **A rule every platform applies the same way** goes in `crates/core/src/policy.rs`, written over plain data and tested there: which apps are password managers or Chromium browsers (`is_password_manager`, `is_chromium_browser`, with `Os::CURRENT`), whether a selection is still the one captured (`selection_unchanged` over two `Selection`s), and which ranges a review leaves as written (`protected_ranges`). A platform module supplies what it can read and calls these.
- **Settings** are `Config` in `crates/core/src/config.rs`. It is `#[serde(default)]`, so settings saved by an older version load.
- **Platform code** is in `src-tauri/src/platform/{macos,linux,windows}/`, one file per topic. `src-tauri/src/platform/mod.rs` lists what every platform module provides, with the same names and signatures, and re-exports the module for the system being built, so the rest of the app calls `platform::name` without a `cfg`. `unsupported.rs` stands in on any other system.
- **A review's decisions** are methods of `Controller` in `src-tauri/src/review.rs`, generic over the traits in `src-tauri/src/review/shell.rs`: `Shell` (the panel and its shortcuts), `Host` (the platform, the reviewed app and the clock) and `Engine`. `Live` implements them for the app; the public functions in `review.rs` take an `AppHandle`, build a `Live` controller and call it. Anything new a review does to the outside world becomes a method on one of these traits.
- **Commands the windows invoke** are in `src-tauri/src/commands.rs`, registered in `generate_handler!` in `src-tauri/src/lib.rs`.

## Contracts between Rust and the UI

Each contract is a file both sides depend on, checked by a test on each side. `scripts/check.sh` runs them all. The fixtures are in `tests/contracts/`.

| Contract | Rust side | Other side |
|---|---|---|
| Payload types in `ui/src/lib/generated/`, written by ts-rs from Rust types | `cargo test --workspace` writes them | `check.sh` fails when the committed files differ |
| Command and event names: `ipc-names.json` | `src-tauri/src/ipc_contract.rs` | `ui/src/lib/ipc-names.ts`, checked by `ui/src/lib/contracts.test.ts` |
| Recorded shortcut strings: `accelerators.json` | `src-tauri/src/accelerator_contract.rs` | `ui/src/lib/contracts.test.ts` |
| Diff views and the text rejected changes rebuild: `diff-apply.json` | `shared_fixture_views_and_rejections` in `crates/core/src/diff.rs` | `ui/src/lib/contracts.test.ts` |
| Words the live UI drivers look for: `ui-wording.json` | `src-tauri/src/ui_wording_contract.rs` | `ui/src/lib/ui-wording.svelte.test.ts` |
| `capture.log` lines the live UI drivers read: `log-lines.json` | `src-tauri/src/platform/log_lines/tests.rs` | the drivers' patterns, checked against their source by the same tests |

## Changes that travel together

- **A Tauri command or event**: the function in `commands.rs` and its entry in `generate_handler!` (or the `emit`), its name in `tests/contracts/ipc-names.json` and `ui/src/lib/ipc-names.ts`, and the call in `ui/src/settings/ipc.ts` or `ui/src/overlay/ipc.ts`. A new payload type derives `ts_rs::TS` with `ts(export)`, as those in `commands.rs` do; run `cargo test --workspace` and commit what it writes to `ui/src/lib/generated/`.
- **A setting**: the field and its default in `Config`, then `cargo test --workspace` to rewrite `ui/src/lib/generated/Config.ts`. What saving a change to it must do goes in `commands.rs`, tested in `src-tauri/src/commands/tests.rs`.
- **Wording in a window or menu**: if `tests/contracts/ui-wording.json` lists it, change the fixture and the driver that matches it (`tests/ui/smoke.swift`, `tests/ui/linux/`, `tests/ui/windows/`) in the same change. The same goes for a `capture.log` line in `tests/contracts/log-lines.json`, whose wording lives in `src-tauri/src/platform/log_lines.rs`.
- **A password manager**: a row in `PASSWORD_MANAGERS` in `policy.rs`, with its ID on each system, and that ID in the system's `DEFAULT_EXCLUSIONS` in `config.rs`; `every_password_manager_is_excluded_by_default` fails without it. The README's Privacy section names them.
- **A platform behaviour**: in that platform's module, with a test against its pretend desktop (`testing.rs` in each platform folder). A function the rest of the app calls goes in every platform module and `unsupported.rs`, and in the list in `platform/mod.rs`. A new file that belongs to one platform only goes in that platform's row of `docs/versioning.md` and its label in `.github/labeler.yml`.

## Versions

Before bumping the version or preparing a release, read `docs/versioning.md`: one version line for every platform, and each release ships only the platforms whose code changed since that platform's last stable release.

## The Omarchy bar widget

`packaging/omarchy/` is published, as it is, to its own repository by each release that ships Linux. Before changing it or how it's published, read `docs/omarchy-plugin.md`.

## Markdown

Write each paragraph and list item on one line, and let the editor wrap it. A hard-wrapped paragraph turns a two-word edit into a diff of the whole paragraph.

## Everything here is public

Every file in this repository is public, and so is every commit message, branch name, pull request and issue. Write every line, comment and test so a stranger can read it:

- **Example data is invented.** Use made-up names (Rhyan, Brynlee, Kyndal), addresses ("1234 Glenmoor Ct"), organisations (QuillPoint) and generic acronyms (KPI, OKR); real people, places, employers and accounts stay out.
- **Describe what the code does today.** SuperEditr's own prices, paid tiers, licensing plans and roadmap, and comparisons with competing products, stay out of code, comments, tests, commit messages and pull requests. Facts about AI providers, such as their published pricing, are fine.
- **Name infrastructure generically**: "a CI runner", "the signing machine", "a maintainer's Mac". Non-public hostnames, machine names and IP addresses, home-directory paths, internal tool names and private repository names stay out. Public service endpoints, `localhost` and invented examples (`db.internal`) are fine.
- **Credentials appear only as obviously fake fixtures**, such as `sk-proj-0123456789`. A test that needs a full-length key builds it from pieces at run time, as `crates/core/src/sensitive/tests.rs` does, so no complete key appears in the source for secret scanners to flag.
- **Commit messages say what changed in plain words.** Context that isn't public goes in no commit or pull request.

Before committing, read `git diff --staged` as if it were already published.
