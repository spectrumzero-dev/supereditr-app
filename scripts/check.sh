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
# The tests write ui/src/lib/generated, the TypeScript definitions of the IPC payloads, from the Rust
# types (src-tauri/src/ipc_contract.rs). The files are compared with the ones there before, and
# cleared first so that a definition no type writes any more shows too. `git diff --no-index` needs
# no repository, and the files are checked out with LF line endings everywhere (.gitattributes).
generated=ui/src/lib/generated
before=target/ipc-types-before
rm -rf "$before"
mkdir -p "$before"
cp "$generated"/*.ts "$before"/ 2> /dev/null || true
rm -f "$generated"/*.ts
cargo test --workspace
if ! git diff --no-index --stat --exit-code -- "$before" "$generated"; then
  echo "$generated was out of date. cargo test --workspace has just rewritten it from the Rust types: commit the changes." >&2
  exit 1
fi
rm -rf "$before"
npm --prefix ui run check
npm --prefix ui test
npm --prefix ui run build
node --test tests/omarchy/model.test.mjs tests/versions.test.mjs
echo "All checks passed."
