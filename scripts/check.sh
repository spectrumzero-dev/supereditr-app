#!/usr/bin/env bash
# Runs every automated check: formatting, lints, Rust tests, UI type check and tests.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
[ -d ui/node_modules ] || npm --prefix ui ci
if command -v shellcheck > /dev/null; then shellcheck -S warning scripts/*.sh; fi
# The live UI tests run only on a Mac with a display (scripts/ui-smoke.sh); here they must compile.
if [[ "$(uname)" == Darwin ]]; then xcrun swiftc -typecheck tests/ui/smoke.swift; fi
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm --prefix ui run check
npm --prefix ui test
npm --prefix ui run build
echo "All checks passed."
