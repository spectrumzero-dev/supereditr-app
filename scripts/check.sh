#!/usr/bin/env bash
# Runs every automated check: formatting, lints, Rust tests, UI type check and tests.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
[ -d ui/node_modules ] || npm --prefix ui ci
# CI's Linux job sets REQUIRE_SHELLCHECK, so the scripts are checked on every change there.
if command -v shellcheck > /dev/null; then
  shellcheck -S warning scripts/*.sh tests/ui/linux/*.sh
elif [[ -n "${REQUIRE_SHELLCHECK:-}" ]]; then
  echo "shellcheck isn't installed." >&2
  exit 1
fi
# The live UI tests run only on a Mac with a display (scripts/ui-smoke.sh); here they must compile.
if [[ "$(uname)" == Darwin ]]; then xcrun swiftc -typecheck tests/ui/smoke.swift; fi
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
# The tests write the TypeScript definitions of the IPC payloads from the Rust types
# (src-tauri/src/ipc_contract.rs). Here they write to a fresh directory, which is compared with the
# committed ui/src/lib/generated: a failed build or test leaves the committed files alone, and a
# definition no type writes any more shows as a difference too. `git diff --no-index` needs no
# repository, and the files are checked out with LF line endings everywhere (.gitattributes).
generated=ui/src/lib/generated
fresh=target/ipc-types
rm -rf "$fresh"
mkdir -p "$fresh"
# An absolute path, in Windows form under Git Bash: each crate's tests run in its own directory.
TS_RS_EXPORT_DIR="$(pwd -W 2> /dev/null || pwd)/$fresh" cargo test --workspace
if ! git diff --no-index --stat --exit-code -- "$generated" "$fresh"; then
  rm -f "$generated"/*.ts
  cp "$fresh"/*.ts "$generated"/
  echo "$generated was out of date and has been rewritten from the Rust types: commit the changes." >&2
  exit 1
fi
rm -rf "$fresh"
npm --prefix ui run check
npm --prefix ui test
npm --prefix ui run build
node --test tests/omarchy/model.test.mjs tests/versions.test.mjs
echo "All checks passed."
