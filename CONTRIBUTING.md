# Contributing to SuperEditr

Bug reports, fixes and improvements are welcome. Open an issue first for anything large, so we can
agree on the approach before you spend time on it.

## Build and check

Requirements and build steps are in the [README](README.md#build-from-source). Run every check
before you open a pull request:

```bash
./scripts/check.sh
```

It runs formatting, Clippy with warnings as errors, the Rust tests, and the UI type check, tests and
build. CI runs the same script on every pull request.

Code that captures or replaces text in other apps lives in `src-tauri/src/platform/`. Changes there
are hard to test automatically, so say in the pull request which apps you tried and on which macOS
version.

### Secret rules

The rules that keep secrets out of reviews are a table in
[`crates/core/src/sensitive.rs`](crates/core/src/sensitive.rs), one row per format, naming the
document that defines the format where there is one; the general rules for passwords and labelled
keys are heuristics. To add one, add a row there and cases in
`crates/core/src/sensitive/tests.rs`: a text the rule must catch, and a lookalike it must leave alone.
The tests fail when a rule has no case that catches it. A rule has to stay quiet on ordinary writing
(commit hashes, UUIDs, placeholders like `your-api-key`), so prefer a fixed prefix over a broad
pattern. Never put a real secret in a test: assemble made-up ones from pieces, as the existing cases
do, so secret scanners don't flag the source.

## How a pull request lands

CI checks every pull request. A maintainer reviews it, may ask for changes, and merges it here once
it's ready. Keep your branch up to date with `main`, and keep each pull request to one change.

## Licence

SuperEditr is licensed under the [Apache License 2.0](LICENSE). As section 5 of the licence says,
anything you submit is licensed under the same terms unless you state otherwise. There is no
contributor agreement to sign.

## Free and paid builds

Everything in this repository is free when you build it yourself. Official signed builds from
supereditr.app may put some features, including contributed ones, in a paid tier. Pull requests that
change how official builds are licensed or priced won't be merged.

## Name and logo

The code and icon files are licensed under Apache-2.0, but the licence grants no rights to use the
SuperEditr name or logo as a trademark. If you publish your own build, see
[TRADEMARKS.md](TRADEMARKS.md).

## Security

Report vulnerabilities privately, as described in [SECURITY.md](SECURITY.md), not in a public issue.
