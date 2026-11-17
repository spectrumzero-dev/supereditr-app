#!/usr/bin/env bash
# Runs the AI edit eval (crates/eval) for one provider, then scores the results.
#
#   scripts/eval.sh PROVIDER [OUT_DIR] [-- extra `supereditr-eval run` flags]
#
# PROVIDER is apple, anthropic, openai, gemini, ollama, compatible or claude-cli. Results go to
# OUT_DIR (default target/eval/<provider>-<timestamp>): results.jsonl, report.md and summary.json.
# Set EVAL_BASELINE to a summary.json to compare against it, and EVAL_FAIL_ON_REGRESSION=1 to exit
# with status 1 when the comparison flags a regression or finds a group with no baseline. For
# Apple Intelligence the helper is compiled from helpers/foundation-models first.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Relative paths are the caller's, so resolve them before changing directory.
if [[ -n "${EVAL_BASELINE:-}" && "$EVAL_BASELINE" != /* ]]; then
  EVAL_BASELINE="$PWD/$EVAL_BASELINE"
fi
if [[ -n "${EVAL_FAIL_ON_REGRESSION:-}" && -z "${EVAL_BASELINE:-}" ]]; then
  echo "EVAL_FAIL_ON_REGRESSION needs EVAL_BASELINE, the summary to compare with." >&2
  exit 2
fi
cd "$ROOT"
provider="${1:?usage: scripts/eval.sh PROVIDER [OUT_DIR] [-- run flags]}"
shift
out="target/eval/${provider}-$(date -u +%Y%m%dT%H%M%SZ)"
if [[ $# -gt 0 && "$1" != "--" ]]; then
  out="$1"
  shift
fi
[[ "${1:-}" == "--" ]] && shift
mkdir -p "$out"

if [[ "$provider" == apple ]]; then
  helper="$ROOT/target/eval/supereditr-fm"
  mkdir -p "$(dirname "$helper")"
  arch="$(uname -m)"
  xcrun swiftc -O -swift-version 5 -target "${arch}-apple-macos13.0" \
    -Xlinker -weak_framework -Xlinker FoundationModels \
    helpers/foundation-models/main.swift -o "$helper"
  # The helper's first line says whether the on-device model is available on this Mac.
  hello="$(: | "$helper" | head -n 1)"
  echo "Apple Intelligence helper: $hello"
  export SUPEREDITR_FM_HELPER="$helper"
fi

SUPEREDITR_EVAL_COMMIT="${SUPEREDITR_EVAL_COMMIT:-$(git rev-parse HEAD)}" \
  cargo run --release -q -p supereditr-eval -- run --provider "$provider" --out "$out/results.jsonl" "$@"

score=(score "$out/results.jsonl" --report "$out/report.md" --summary "$out/summary.json")
if [[ -n "${EVAL_BASELINE:-}" ]]; then
  # A missing file fails the scoring step, so a mistyped path can't pass for "no regressions".
  score+=(--baseline "$EVAL_BASELINE")
  [[ -n "${EVAL_FAIL_ON_REGRESSION:-}" ]] && score+=(--fail-on-regression)
fi
echo "Report: $out/report.md"
cargo run --release -q -p supereditr-eval -- "${score[@]}"
