#!/usr/bin/env bash
# Builds the Linux release tarball, supereditr-<version>-x86_64.tar.gz, in build/, and prints its
# SHA-256. Run it on x86-64 Linux.
#
# The tarball holds the binary, the desktop entry (named for the app ID the GlobalShortcuts portal
# registers), icons and the licence. build/arch/ gets a copy of packaging/arch/PKGBUILD with this
# version and checksum, its install script, the packaging's 0BSD LICENSE and the tarball, so
# `makepkg -si` there installs it as supereditr-bin. The tracked PKGBUILD is left as it is: its
# version and checksum change only when a release ships Linux (docs/versioning.md).
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
install -Dm755 "${CARGO_TARGET_DIR:-target}/release/supereditr" "$DIR/supereditr"
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
rm -rf build/arch
mkdir -p build/arch
sed -e "s/^pkgver=.*/pkgver=$VERSION/" -e "s/^sha256sums=.*/sha256sums=('$SUM')/" packaging/arch/PKGBUILD > build/arch/PKGBUILD
cp packaging/arch/supereditr.install packaging/arch/LICENSE "$TARBALL" build/arch/
echo "$SUM  $TARBALL"
