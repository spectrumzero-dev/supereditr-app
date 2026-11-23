# Licence and notice files from upstream repositories

`scripts/third-party-notices.py` uses these files offline. Each was fetched once from the place below, at the commit or release shown, and is kept unchanged.

## Crates whose archives ship no licence file

`scripts/third-party-upstream.json` lists, for each repository, the files taken from it and the commit each crate was published from, as recorded in the crate archive's `.cargo_vcs_info.json`. The generator checks that commit against the archive, so a crate update fails until its licence files are checked again. Where several crates were published from different commits, the files were identical at every one of them.

| Folder | Repository | Files |
|---|---|---|
| `alloc-no-stdlib` | https://github.com/dropbox/rust-alloc-no-stdlib | `LICENSE` |
| `burn` | https://github.com/tracel-ai/burn | `LICENSE-APACHE`, `LICENSE-MIT`, `NOTICES.md` |
| `cubecl` | https://github.com/tracel-ai/cubecl | `LICENSE-APACHE`, `LICENSE-MIT` |
| `dlopen2` | https://github.com/OpenByteDev/dlopen2 | `LICENSE` |
| `embassy` | https://github.com/embassy-rs/embassy | `LICENSE-APACHE`, `LICENSE-MIT` |
| `harper` | https://github.com/automattic/harper | `LICENSE` |
| `libappindicator-rs` | https://github.com/tauri-apps/libappindicator-rs | `LICENSE-APACHE`, `LICENSE-MIT` |
| `objc2` | https://github.com/madsmtm/objc2 | `LICENSE.md` |
| `webview2-rs` | https://github.com/wravery/webview2-rs | `LICENSE` |
| `whatlang-rs` | https://github.com/greyblake/whatlang-rs | `LICENSE` |

objc2's `LICENSE.md` names each crate's licences without their texts, so the standard SPDX texts are shown after it (`standardTexts` in the manifest).

A crate whose repository has no licence file at all goes in `withoutLicence` in the manifest, with the reason; it then shows the standard SPDX text.

The generator checks the manifest before using it: each project lists at least one licence file, every listed file exists, and every `withoutLicence` entry gives its reason. A crate's own notice files are shown beside those taken from its repository, and files from one repository are named by their path below the folder holding them, so no file replaces another with the same name. Settings labels each file with where it came from: the repository at that commit, or a standard SPDX text. `tests/third_party_notices_test.py` covers these checks.

## Other components

- `rust`: `COPYRIGHT`, `LICENSE-APACHE` and `LICENSE-MIT` from https://github.com/rust-lang/rust at tag `1.99.1` (commit `58cf8f9c35b355199727627d07ae34a4e2846d67`), the toolchain in `rust-toolchain.toml`. The standard library is linked into every build. When the toolchain changes, fetch these from the new tag and update the version in `scripts/third-party-extra.json`; the generator fails until they match.
- `webview2-sdk`: `LICENSE.txt` and `NOTICE.txt` from the Microsoft.Web.WebView2 1.0.3800.47 NuGet package, https://www.nuget.org/packages/Microsoft.Web.WebView2/1.0.3800.47. On Windows (MSVC), `webview2-com-sys` 0.39.1 links `WebView2LoaderStatic.lib` from that SDK version into the app. Its entry in `scripts/third-party-extra.json` names that crate version in `shippedWith`, so the generator fails when the crate changes.
