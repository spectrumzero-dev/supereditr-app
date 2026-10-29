#!/usr/bin/env bash
# Counts the app's own lines of code, by language: the engine, the Mac app, the UI and the
# Apple Intelligence helper. Leaves out blank lines, comments, tests (Rust `mod tests` blocks,
# `tests.rs` files, `*.test.ts` and `tests/`), build scripts, config files and dependencies.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

count() {
  # $1: language label; the remaining arguments: files.
  local label="$1"
  shift
  awk '
    FNR == 1 { skip = 0; endc = "" }
    # A Rust test module starts at column 0 and ends at the next closing brace in column 0.
    /^#\[cfg\(test\)\]/ { pending = 1; next }
    pending && /^mod tests \{/ { skip = 1; pending = 0; next }
    pending { pending = 0 }
    skip { if (/^\}/) skip = 0; next }
    # Inside a block comment, skip lines until the one that closes it.
    endc != "" { if (index($0, endc)) endc = ""; next }
    /^[[:space:]]*$/ { next }
    /^[[:space:]]*\/\// { next }
    /^[[:space:]]*\/\*/ { if (!index($0, "*/")) endc = "*/"; next }
    /^[[:space:]]*<!--/ { if (!index($0, "-->")) endc = "-->"; next }
    { n++ }
    END { print n + 0 }
  ' "$@" | { read -r n; printf '%-8s %6d\n' "$label" "$n"; echo "$n" >> "$TOTALS"; }
}

TOTALS="$(mktemp)"
trap 'rm -f "$TOTALS"' EXIT

files() { git ls-files -- "$@"; }
rust=$(files 'crates/*.rs' 'src-tauri/src/*.rs' | grep -v '/tests\.rs$' | grep -v '^crates/[^/]*/tests/')
svelte=$(files 'ui/src/*.svelte')
ts=$(files 'ui/src/*.ts' | grep -v '\.test\.ts$')
css=$(files 'ui/src/*.css')
swift=$(files 'helpers/*.swift')

# shellcheck disable=SC2086 # word splitting is intended: the lists hold paths without spaces
{
  count Rust $rust
  count Svelte $svelte
  count TS $ts
  count CSS $css
  count Swift $swift
}
printf '%-8s %6d\n' Total "$(paste -sd+ "$TOTALS" | bc)"
