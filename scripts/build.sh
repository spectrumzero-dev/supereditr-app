#!/usr/bin/env bash
# Builds SuperEditr.app and a drag-to-install DMG in build/.
#
# Signing: SUPEREDITR_SIGNING_IDENTITY, a certificate name from `security find-identity -v -p
# codesigning`, or ad-hoc, after which macOS asks for Accessibility permission again on each rebuild.
#
# SUPEREDITR_SOURCE_SHA, when set, is recorded in the app's Info.plist as SuperEditrBuildCommit, so a
# build made elsewhere from this commit can show which source it came from.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
[ -d ui/node_modules ] || npm --prefix ui ci

PLIST=src-tauri/Info.plist
STAGE=""
PLIST_BACKUP=""
cleanup() {
  if [[ -n "$STAGE" ]]; then rm -rf "$STAGE"; fi
  if [[ -n "$PLIST_BACKUP" ]]; then mv "$PLIST_BACKUP" "$PLIST"; fi
}
trap cleanup EXIT
if [[ -n "${SUPEREDITR_SOURCE_SHA:-}" ]]; then
  [[ "$SUPEREDITR_SOURCE_SHA" =~ ^[0-9a-f]{40}$ ]] || { echo "SUPEREDITR_SOURCE_SHA must be a full commit hash" >&2; exit 1; }
  backup="$(mktemp)"
  cp "$PLIST" "$backup"
  PLIST_BACKUP="$backup" # set only once the copy exists, so a failed copy never replaces the plist
  plutil -remove SuperEditrBuildCommit "$PLIST" 2> /dev/null || true
  plutil -insert SuperEditrBuildCommit -string "$SUPEREDITR_SOURCE_SHA" "$PLIST"
fi

export APPLE_SIGNING_IDENTITY="${SUPEREDITR_SIGNING_IDENTITY:--}"
./ui/node_modules/.bin/tauri build --bundles app

APP="${CARGO_TARGET_DIR:-target}/release/bundle/macos/SuperEditr.app"
VERSION="$(plutil -extract CFBundleShortVersionString raw "$APP/Contents/Info.plist")"
codesign --verify --deep --strict "$APP"

mkdir -p build
STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
DMG="build/SuperEditr-$VERSION.dmg"
rm -f "$DMG"
hdiutil create -volname "SuperEditr" -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov "$DMG" >/dev/null
rm -rf "build/SuperEditr.app"
cp -R "$APP" build/
echo "Built build/SuperEditr.app and $DMG ($(du -h "$DMG" | cut -f1))."
