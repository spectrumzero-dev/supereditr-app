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
# The Windows runner exposes Python as python.exe in Git Bash.
python=python3
if [[ "$(uname)" == MINGW* ]]; then
  python=python
fi
notices=ui/src/settings/third-party-notices.json
mkdir -p target
fresh="$(mktemp -d target/third-party-notices.XXXXXX)"
trap 'rm -rf "$fresh"' EXIT
"$python" -I scripts/third-party-notices.py --output "$fresh/third-party-notices.json"
if ! git diff --no-index --stat --exit-code -- "$notices" "$fresh/third-party-notices.json"; then
  echo "$notices is out of date. Run $python -I scripts/third-party-notices.py." >&2
  exit 1
fi
rm -rf "$fresh"
trap - EXIT
# The live UI tests run only on a Mac with a display (scripts/ui-smoke.sh); here they must compile.
if [[ "$(uname)" == Darwin ]]; then xcrun swiftc -typecheck tests/ui/smoke.swift; fi
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
# The tests write the TypeScript definitions of the IPC payloads from the Rust types
# (src-tauri/src/ipc_contract.rs). Here they write to a directory of this run's own, which is
# compared with the committed ui/src/lib/generated and never changes it: a definition no type writes
# any more shows as a difference too. `git diff --no-index` needs no repository, and the files are
# checked out with LF line endings everywhere (.gitattributes).
generated=ui/src/lib/generated
fresh="$(mktemp -d target/ipc-types.XXXXXX)"
trap 'rm -rf "$fresh"' EXIT
# An absolute path, in Windows form under Git Bash: each crate's tests run in its own directory.
TS_RS_EXPORT_DIR="$(pwd -W 2> /dev/null || pwd)/$fresh" cargo test --workspace
if ! git diff --no-index --stat --exit-code -- "$generated" "$fresh"; then
  echo "$generated is out of date. To rewrite it from the Rust types: rm $generated/*.ts && cargo test --workspace" >&2
  exit 1
fi
npm --prefix ui run check
npm --prefix ui test
npm --prefix ui run build
node --test tests/omarchy/model.test.mjs tests/versions.test.mjs
echo "All checks passed."
