# Versions and releases

SuperEditr has one version line for every platform. A version number names a snapshot of the code, and each release ships only the platforms whose shipped code changed in that snapshot. A Linux fix doesn't send Mac users an update; the next release that changes the Mac takes them straight past the skipped number.

## The version

One pull request sets the version in the workspace `Cargo.toml`, `src-tauri/tauri.conf.json` and `ui/package.json`, and their lock files. Every platform's build reports that version. Versions only go up by SemVer precedence (`0.1.6-rc.1` before `0.1.6`), and a number is never reused, so a version one platform skips stays skipped there.

## Which platforms a release ships

Decide each platform on its own, by comparing the release's commit with the platform's **baseline**: the newest published, stable (not pre-release) release whose assets include that platform's build, `SuperEditr-<version>.dmg` for macOS, `supereditr-<version>-x86_64.tar.gz` for Linux or `SuperEditr_<version>_x64-setup.exe` for Windows. A pre-release never moves a baseline. A platform with no baseline yet ships its first build.

```bash
gh release list --exclude-pre-releases --exclude-drafts
gh release view v0.1.5 --json assets -q '.assets[].name'
git diff --name-status --no-renames v0.1.5 <release-commit>
```

The platform ships when the diff touches a file in its row or the shared row. `--no-renames` lists a moved file as a deletion and an addition, so both its old and new places count.

| Class | Files |
|---|---|
| macOS | `src-tauri/src/platform/macos.rs`, `src-tauri/Info.plist`, `src-tauri/tauri.macos.conf.json`, `src-tauri/icons/icon.icns`, `src-tauri/icons/64x64.png`, `helpers/foundation-models/`, `scripts/build.sh` |
| Linux | `src-tauri/src/platform/linux/`, `src-tauri/src/tray_linux.rs`, `src-tauri/icons/tray-light.png`, `packaging/arch/`, `packaging/linux/`, `scripts/linux-release.sh` |
| Windows | `src-tauri/src/platform/windows/`, `src-tauri/tauri.windows.conf.json`, `src-tauri/windows/`, `src-tauri/icons/icon.ico` |
| Not shipped | Markdown files; the Omarchy bar widget (`packaging/omarchy/`), which is published to its own repository rather than in a release; tests (`tests/`, `scripts/ui-smoke*.sh`, Rust `tests` modules and files, UI `*.test.ts`); CI (`.github/`); development tools (`scripts/check.sh`, `scripts/count-lines.sh`, formatter and linter configuration) |
| Release bookkeeping | a change to the version alone, in the files above, and the version and SHA-256 lines of `packaging/arch/PKGBUILD` |
| Shared | everything else |

Shared ships every platform. It covers code with platform branches, such as `lib.rs` and `crates/core/src/config.rs`, because a change there can alter any platform's build, and every file outside the other rows: a file nobody has classified counts as shared. Bookkeeping counts only when it is the whole change; a dependency change in `Cargo.toml` or a lock file is shared. When a new file belongs to one platform only, add it to that platform's row, and to that platform's label in `.github/labeler.yml`, in the same pull request.

The version pull request lists every platform with its baseline and whether it ships ("macOS: ships, baseline v0.1.5. Linux: skips, baseline v0.1.6"). The release title names the platforms it ships ("SuperEditr 0.2.0 for macOS and Linux"), and its notes start with them ("**Platforms:** macOS, Linux"). GitHub groups the generated notes by the labels `.github/labeler.yml` gives each pull request from the table above; the version pull request carries the `release` label, which leaves it out of the notes.

## Updates on each platform

- **macOS** apps check `latest.json` on the repository's release marked Latest. That mark stays on the newest release that ships macOS: a release that leaves out macOS is published without it, so Mac apps keep finding the last macOS release and its download link keeps working. Older Mac installs can still update to that release.
- **Windows** apps ask the update server for the newest stable release that includes a Windows installer, so a release that leaves out Windows doesn't change what they see, and one that ships only Windows reaches them without being marked Latest.
- **Linux** has no in-app updater. `packaging/arch/PKGBUILD` names a release and the SHA-256 of its tarball, written after that release's tarball is built, as release bookkeeping.
