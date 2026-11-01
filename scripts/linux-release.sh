#!/usr/bin/env bash
# Builds the Linux release tarball, supereditr-<version>-x86_64.tar.gz, in build/, prints its
# SHA-256, and writes that into packaging/arch/PKGBUILD. Run it on x86-64 Linux.
#
# The tarball holds the binary, the desktop entry (named for the app ID the GlobalShortcuts portal
# registers), icons and the licence. packaging/arch/PKGBUILD installs it as supereditr-bin.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
[[ "$(uname -s)-$(uname -m)" == Linux-x86_64 ]] || { echo "Build the Linux release on x86-64 Linux." >&2; exit 1; }
[ -d ui/node_modules ] || npm --prefix ui ci

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
NAME="supereditr-$VERSION-x86_64"
./ui/node_modules/.bin/tauri build --no-bundle

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
DIR="$STAGE/$NAME"
install -Dm755 target/release/supereditr "$DIR/supereditr"
install -Dm644 packaging/linux/app.supereditr.desktop "$DIR/app.supereditr.desktop"
install -Dm644 src-tauri/icons/32x32.png "$DIR/icons/32x32.png"
install -Dm644 src-tauri/icons/128x128.png "$DIR/icons/128x128.png"
install -Dm644 src-tauri/icons/128x128@2x.png "$DIR/icons/256x256.png"
install -Dm644 LICENSE "$DIR/LICENSE"

mkdir -p build
TARBALL="build/$NAME.tar.gz"
# Fixed owners, order and times, so the same binary gives the same tarball.
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@${SOURCE_DATE_EPOCH:-0}" \
  -C "$STAGE" -cf - "$NAME" | gzip -n -9 > "$TARBALL"
SUM="$(sha256sum "$TARBALL" | cut -d' ' -f1)"
sed -i -e "s/^pkgver=.*/pkgver=$VERSION/" -e "s/^sha256sums=.*/sha256sums=('$SUM')/" packaging/arch/PKGBUILD
echo "$SUM  $TARBALL"
