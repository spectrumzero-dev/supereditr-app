# Contributing to SuperEditr

Bug reports, fixes and improvements are welcome. Open an issue first for anything large, so we can agree on the approach before you spend time on it.

## Build and check

Requirements and build steps are in the [README](README.md#build-from-source). Run every check before you open a pull request:

```bash
./scripts/check.sh
```

It runs formatting, Clippy with warnings as errors, the Rust tests, a check that the UI's generated IPC types match the Rust ones, and the UI type check, tests and build. CI runs the same script on macOS, Linux and Windows on every pull request. [Testing](docs/testing.md) describes the test layers and where a new test belongs: at the lowest layer that can catch the bug.

Code that captures or replaces text in other apps lives in `src-tauri/src/platform/`, with a module per system. Each has a pretend desktop (`testing.rs`) that its capture and replace decisions are tested against, but how real apps answer shows only in the apps themselves, so say in the pull request which apps you tried and on which macOS version, Windows version or Linux desktop.

### Secret rules

The local secret check imports rules from [Gitleaks v8.30.1](https://github.com/gitleaks/gitleaks/releases/tag/v8.30.1). Imported patterns require a distinctive leading literal and token boundaries. The [generated file header](crates/core/src/sensitive/gitleaks.rs) gives the import and exclusion counts and lists the reasons for exclusions, including rules that rely on a vendor keyword in surrounding text. The hand-written rows in [sensitive.rs](crates/core/src/sensitive.rs) cover additional formats and keep precedence when both tables recognize a format. The password and labelled-key heuristics run after the named rules.

Run `./scripts/sync-gitleaks.py` with Python 3.11+ and the repository Rust toolchain to regenerate the rules and their MIT license. When updating the release, update the tag and verify both upstream checksums in the script. Review the generated rules and exclusion reasons, then run `./scripts/check.sh`. The app builds from the checked-in rules without fetching Gitleaks.

For a new hand-written format, add a caught case and a lookalike in `crates/core/src/sensitive/tests.rs`. Every imported rule also has a generated caught case. The tests fail when a rule has no case that it catches. Prefer a fixed prefix over a broad pattern. Assemble made-up values from pieces at runtime. The rules should stay quiet on ordinary writing such as commit hashes, UUIDs, and placeholders like `your-api-key`.

## How a pull request lands

CI checks every pull request. A maintainer reviews it, may ask for changes, and merges it here once it's ready. Keep your branch up to date with `main`, and keep each pull request to one change.

## Licence

SuperEditr is licensed under the [Apache License 2.0](LICENSE). As section 5 of the licence says, anything you submit is licensed under the same terms unless you state otherwise. There is no contributor agreement to sign.

## Free and paid builds

Everything in this repository is free when you build it yourself. Official signed builds from supereditr.app may put some features, including contributed ones, in a paid tier. Pull requests that change how official builds are licensed or priced won't be merged.

## Name and logo

The code and icon files are licensed under Apache-2.0, but the licence grants no rights to use the SuperEditr name or logo as a trademark. If you publish your own build, see [TRADEMARKS.md](TRADEMARKS.md).

## Security

Report vulnerabilities privately, as described in [SECURITY.md](SECURITY.md), not in a public issue.
