#!/bin/bash
# Runs the live UI smoke tests (tests/ui/smoke.swift) against a built app, on a Mac with an unlocked
# display. Screenshots land in OUT_DIR. When the Mac can't run UI tests (locked screen, missing
# permission, another SuperEditr running), it warns and exits 0, so a build that runs it still succeeds.
#
#   scripts/ui-smoke.sh [APP] [OUT_DIR]     defaults: build/SuperEditr.app build/ui-smoke
#   scripts/ui-smoke.sh APP OUT_DIR --accessibility-cycle
# In a disposable VM, open APP normally, finish onboarding and close its Settings window.
# Run the second command from an independently trusted driver. Toggle only SuperEditr's
# Accessibility switch when prompted. The driver checks input and recovery in the same process.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="$(cd "$(dirname "${1:-$ROOT_DIR/build/SuperEditr.app}")" && pwd)/$(basename "${1:-$ROOT_DIR/build/SuperEditr.app}")"
OUT_DIR="${2:-$ROOT_DIR/build/ui-smoke}"
[[ -x "$APP/Contents/MacOS/supereditr" ]] || { echo "ui-smoke: app not found: $APP" >&2; exit 1; }

tool="$(mktemp -d)/smoke"
trap 'rm -rf -- "$(dirname "$tool")"' EXIT
xcrun swiftc -O "$ROOT_DIR/tests/ui/smoke.swift" -o "$tool"
# Wake the display if it dimmed; this asserts user activity and changes no settings.
caffeinate -u -t 2 || true

status=0
"$tool" "$APP" "$OUT_DIR" "${@:3}" || status=$?
# The driver writes the seconds each section took to OUT_DIR/timings.tsv; in CI, the job's summary
# shows them too.
if [[ -n "${GITHUB_STEP_SUMMARY:-}" && -f "$OUT_DIR/timings.tsv" ]]; then
  { printf '### macOS UI smoke tests: seconds per section, slowest first\n\n| Section | Seconds |\n| --- | ---: |\n'
    awk -F '\t' '{ printf "| %s | %s |\n", $1, $2 }' "$OUT_DIR/timings.tsv"; } >> "$GITHUB_STEP_SUMMARY" || true
fi
case "$status" in
  0) ;;
  78)
    [[ "${3:-}" != --accessibility-cycle ]] || exit 1
    message="UI smoke tests skipped: this Mac can't run them right now (see the log)."
    if [[ -n "${GITHUB_ACTIONS:-}" ]]; then echo "::warning title=UI smoke tests skipped::$message"; else echo "$message"; fi
    ;;
  *) exit "$status" ;;
esac
