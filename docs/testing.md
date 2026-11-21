# Testing SuperEditr

SuperEditr's tests come in layers, from pure functions to the app driving real apps on a real desktop. The lower layers run in seconds on any machine; the top one needs a desktop of its own and is slow.

## The rule

Put each check at the lowest layer that can catch the bug. Add a live UI (smoke) check only for something the user sees that no lower layer can prove: how real apps behave, operating-system permissions, windows and focus, and what a paste actually does.

A bug found by a smoke test usually gets its regression test one or two layers down, where it runs on every pull request.

## The layers

| Layer | What it proves | Where | Runs with |
|---|---|---|---|
| Engine and shared rules | Pure logic: diffing, guardrails, settings, secret rules, the rules every platform applies, providers' wire formats against a local mock server | `crates/core` (unit tests beside the code, `crates/core/tests/`) | `cargo test -p supereditr-core` |
| Platform chains | A platform's capture and replace decisions, against a pretend desktop with a fake clock | `testing.rs` and the `tests` modules in `src-tauri/src/platform/{macos,linux,windows}/` | `cargo test -p supereditr` on that system |
| Review controller and commands | What a review does on each event, against a fake panel, platform and engine; saving settings and keys | `src-tauri/src/review/tests_handlers.rs`, `tests_runs.rs`, `src-tauri/src/commands/tests.rs` | `cargo test -p supereditr` |
| UI | Svelte components and the UI's own logic | `ui/src/**/*.test.ts`, component tests named `*.svelte.test.ts` | `npm --prefix ui test` (vitest) |
| Contracts | That Rust and the UI, or the app and the live UI drivers, agree on names, types, strings and log lines | `tests/contracts/`, with a test on each side (see `AGENTS.md`) | `scripts/check.sh` |
| Live UI (smoke) | The built app, driving real apps on a real desktop | `tests/ui/smoke.swift`, `tests/ui/linux/`, `tests/ui/windows/` | `scripts/ui-smoke.sh`, `scripts/ui-smoke-linux.sh`, `scripts/ui-smoke-windows.sh` |

`scripts/check.sh` runs every layer but the last, on whichever system it runs on; CI runs it on macOS, Linux and Windows for every pull request. A platform's chain tests compile and run only on that platform, so a change to another system's code is first compiled and tested in CI. On a Mac with a warm build, `check.sh` takes about a minute and a half, almost all of it compiling: the Rust tests themselves finish in a few seconds, and vitest in about two. CI's check jobs take two to three minutes each.

The live UI tests run only in the maintainers' full builds. Each run needs a test desktop to itself, so runs queue one at a time, and a run takes far longer than `check.sh`. A pull request gets them once it's ready to land; its author doesn't need to run them.

## Examples

- **A shared rule, in `crates/core`.** Every password manager must be on the default excluded apps. `every_password_manager_is_excluded_by_default` in `crates/core/src/policy.rs` checks the two tables against each other in microseconds. No smoke check could cover every password manager on every system.
- **A timing decision, in a platform chain.** A browser still building its accessibility tree is waited for four seconds and then copied from. `a_tree_that_never_comes_is_waited_for_four_seconds_then_copied` in `src-tauri/src/platform/macos/capture.rs` runs it against the pretend Mac in `testing.rs`, whose clock moves only while SuperEditr waits, so the four seconds take none.
- **A failure while writing, in the controller.** `a_failed_write_still_says_so_over_a_review_that_began_meanwhile` in `src-tauri/src/review/tests_handlers.rs` sets the fake host's write to fail and opens a second review during that write (`while_writing`), then checks the calls made and that the failure's notice takes the panel. A race like this is hard to produce on a real desktop and easy to set up against the fake.
- **A row in Settings, in vitest.** `ui/src/settings/Privacy.svelte.test.ts` mounts the Privacy page with a mocked app and checks that a password manager's row has no Remove button.
- **What only a real desktop shows, in a smoke test.** `tests/ui/smoke.swift` checks that a single ⌘E still reaches the app once the double-press window passes, and that ⌘E E opens the review without either press reaching the app: that depends on macOS delivering real key events to a real app.

## Changing what a smoke test reads

The live UI drivers find their way by the words in SuperEditr's windows and by lines in `capture.log`. Both are pinned by contracts (`tests/contracts/ui-wording.json` and `log-lines.json`), so rewording either fails `check.sh` until the fixture and the driver change with it.

`scripts/ui-smoke.sh` and the Linux and Windows scripts describe their requirements and options in their headers; [macOS](macos.md#build-from-source), [Linux](linux.md#build-from-source) and [Windows](windows.md#build-from-source) say what each needs.
