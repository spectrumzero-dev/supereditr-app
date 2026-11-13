#!/usr/bin/env bash
# Runs the Windows UI smoke tests against a Windows test machine, from another computer (the
# controller) that reaches it over SSH as an administrator.
#
# The test machine must be a disposable test machine, never anyone's own computer: the tests turn
# Smart App Control off, install and remove SuperEditr, lock the desktop user's session and connect
# it again through SYSTEM tasks (which skips its password), and can change its display scale and
# restart it.
#
# The tests themselves run on the test machine, inside the signed-in desktop user's session
# (tests/ui/windows): they install SuperEditr from the given installer, then review text in Notepad, Edge, Chrome and Windows Terminal with the
# review shortcut, Alt+Enter, Tab, Space, digits, / and Esc, and look at Settings with no AI and with
# a signed-out Claude Code. Then the races: two identical sentences, the focus moving and the user
# copying while a fix is applied, a paste the app acts on late, and an app that keeps hanging; the
# session locked with the panel open; and WebView2's processes stopping under SuperEditr. No AI
# account and no sign-in is used: reviews come from the built-in rule-based checker. Not covered:
# Word (it needs a licence) and Firefox.
#
#   scripts/ui-smoke-windows.sh --vm USER@HOST --installer FILE --desktop-user NAME [options]
#   scripts/ui-smoke-windows.sh --vm USER@HOST --prepare --desktop-user NAME [options]
#
# For example, against a Proxmox virtual machine with a snapshot prepared for the tests:
#
#   scripts/ui-smoke-windows.sh --vm admin@windows-test --proxmox root@proxmox --vmid 100 \
#     --from-snapshot prepared --desktop-user tests --installer SuperEditr_0.1.7_x64-setup.exe
#
#   --installer FILE     the NSIS installer to test (SuperEditr_x.y.z_x64-setup.exe)
#   --prepare            prepare the test machine for a snapshot that runs start from (see
#                        --from-snapshot) instead of testing, and leave it so: turn Smart App Control
#                        off, compile the helper programs, and open Notepad, Edge, Chrome and Windows
#                        Terminal once in the desktop user's session. Then take the snapshot, with the
#                        machine's memory
#   --desktop-user NAME  the signed-in standard user whose desktop the tests drive
#   --identity FILE      the SSH key for both hosts
#   --out DIR            where results and screenshots go (default build/ui-smoke-windows)
#   --only "CASE ..."    run only these cases: notepad keys edge chrome terminal races lock webview
#                        (install and first-run always run)
#
# When the test machine is a Proxmox virtual machine, these make each run start from a known state
# and leave nothing behind. Both refuse to start while anyone has the machine's console open, since
# the tests drive its desktop:
#
#   --proxmox USER@HOST  the Proxmox host
#   --vmid ID            the virtual machine
#   --from-snapshot NAME roll the virtual machine back to NAME before the run and again after it,
#                        creating and deleting no snapshot. NAME is a snapshot taken with its memory
#                        at the desktop user's signed-in desktop after --prepare, so a run doesn't
#                        pay for that work or for the apps' first start. The usual way to run the
#                        tests
#   --checkpoint         snapshot the virtual machine (with its memory) before the run, then roll
#                        back to it and delete it afterwards: for a machine with no prepared snapshot
#   --smart-app-control-off
#                        turn Smart App Control off before the tests, so an unsigned installer can
#                        run. Needs --from-snapshot or --checkpoint: it can't be turned back on
#   --display-scale PCT  set the desktop user's display scale (100, 150, 200...) and restart before
#                        the tests. Needs --from-snapshot or --checkpoint
#
# The lock case locks the desktop user's session, then asks this script to press the review shortcut
# at the lock screen (with --proxmox, which also photographs the console into OUT/shots), and to
# unlock it by connecting the session to the console again.
#
# Exit status: 0 when every check passed, 1 otherwise. OUT/summary.txt lists every check, OUT/results.tsv
# has one line per check (case, PASS or FAIL, check, milliseconds into the case, detail), OUT/shots
# holds the screenshots and OUT/capture.log SuperEditr's log.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

VM="" INSTALLER="" PREPARE=0 USER_NAME="" IDENTITY="" ONLY="" PROXMOX="" VMID="" FROM="" CHECKPOINT=0 SAC_OFF=0 SCALE=""
OUT="$ROOT/build/ui-smoke-windows"
usage() { sed -n '2,/^set -uo/p' "$0" | sed '$d' >&2; exit 2; }
while (($#)); do
  case "$1" in
    --vm) VM="$2"; shift ;;
    --installer) INSTALLER="$2"; shift ;;
    --desktop-user) USER_NAME="$2"; shift ;;
    --identity) IDENTITY="$2"; shift ;;
    --out) OUT="$2"; shift ;;
    --only) ONLY="$2"; shift ;;
    --proxmox) PROXMOX="$2"; shift ;;
    --vmid) VMID="$2"; shift ;;
    --prepare) PREPARE=1 ;;
    --from-snapshot) FROM="$2"; shift ;;
    --checkpoint) CHECKPOINT=1 ;;
    --smart-app-control-off) SAC_OFF=1 ;;
    --display-scale) SCALE="$2"; shift ;;
    *) usage ;;
  esac
  shift
done
[[ -n "$VM" && (-f "$INSTALLER" || "$PREPARE" == 1) && -n "$USER_NAME" ]] || usage
[[ "$PREPARE" == 0 || (-z "$FROM" && "$CHECKPOINT" == 0) ]] || { echo "--prepare leaves its changes, so it doesn't go with --from-snapshot or --checkpoint" >&2; exit 2; }
[[ "$PREPARE" == 1 ]] && INSTALLER=""
[[ -z "$FROM" || "$CHECKPOINT" == 0 ]] || { echo "--from-snapshot and --checkpoint don't go together" >&2; exit 2; }
ROLLS_BACK=0
[[ -n "$FROM" || "$CHECKPOINT" == 1 ]] && ROLLS_BACK=1
[[ "$ROLLS_BACK" == 0 || (-n "$PROXMOX" && -n "$VMID") ]] || { echo "--from-snapshot and --checkpoint need --proxmox and --vmid" >&2; exit 2; }
[[ "$SAC_OFF" == 0 || "$ROLLS_BACK" == 1 ]] || { echo "--smart-app-control-off needs --from-snapshot or --checkpoint: it can't be undone" >&2; exit 2; }
[[ -z "$SCALE" || "$ROLLS_BACK" == 1 ]] || { echo "--display-scale needs --from-snapshot or --checkpoint" >&2; exit 2; }

REMOTE='C:\SmokeWin'
mkdir -p "$OUT"
rm -rf "${OUT:?}/shots" "$OUT/results.tsv" "$OUT/summary.txt" "$OUT/capture.log" "$OUT/vm"
SSH_OPTS=(-o ConnectTimeout=10 -o ServerAliveInterval=10 -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o LogLevel=ERROR)
[[ -n "$IDENTITY" ]] && SSH_OPTS+=(-i "$IDENTITY" -o IdentitiesOnly=yes)
say() { printf '%s\n' "$*" >&2; }

# SSH that tries again when the connection itself fails (exit 255), never when the command does.
retry_ssh() {
  local rc
  for _ in 1 2 3 4 5 6; do
    "$@"
    rc=$?
    [[ $rc -ne 255 ]] && return $rc
    sleep 3
  done
  return 255
}
# vm COMMAND: Windows PowerShell on the test machine, the SSH server's shell there.
vm() { retry_ssh ssh "${SSH_OPTS[@]}" "$VM" "$1" | tr -d '\r'; }
admin() { vm "powershell -NoProfile -ExecutionPolicy Bypass -File $REMOTE\\admin.ps1 $*"; }
px() { retry_ssh ssh "${SSH_OPTS[@]}" "$PROXMOX" "$@"; }
copy_to() { retry_ssh scp -q "${SSH_OPTS[@]}" "$@"; }

# The snapshot or the checkpoint -----------------------------------------------------------------

SNAPSHOT="" ROLLED_BACK=0
console_open() {
  px "pvesh get /nodes/\$(hostname)/tasks --vmid $VMID --source active --output-format json" |
    python3 -I -c 'import json, sys; print(sum(t.get("type") in ("vncproxy", "vncshell", "spiceproxy", "termproxy") for t in json.load(sys.stdin)))'
}
# Nobody at the console, and no backup holding the machine.
check_free() {
  local open lock
  open="$(console_open)" || { say "Couldn't ask the Proxmox host about consoles"; exit 1; }
  [[ "$open" == 0 ]] || { say "Someone has the console of VM $VMID open; not driving its desktop"; exit 1; }
  lock="$(px "qm config $VMID" | sed -n 's/^lock: //p')"
  [[ -z "$lock" ]] || { say "VM $VMID is locked ($lock), maybe by a backup"; exit 1; }
}
# Rolls back to snapshot $1, and starts the machine if the snapshot has no memory to resume.
rollback_to() {
  px "qm rollback $VMID $1" > /dev/null || return 1
  [[ "$(px "qm status $VMID" | awk '{print $2}')" == running ]] || px "qm start $VMID" > /dev/null
}
from_snapshot() {
  check_free
  px "qm listsnapshot $VMID" | grep -q -- "-> $FROM " || { say "VM $VMID has no snapshot $FROM"; exit 1; }
  say "Rolling VM $VMID back to $FROM"
  rollback_to "$FROM" || { say "The rollback to $FROM failed; not running the tests on whatever VM $VMID holds now"; exit 1; }
  ROLLED_BACK=1
}
take_checkpoint() {
  check_free
  SNAPSHOT="smoke-$(date +%Y%m%d-%H%M%S)"
  say "Checkpoint $SNAPSHOT of VM $VMID, with its memory"
  px "qm snapshot $VMID $SNAPSHOT --vmstate 1 --description 'SuperEditr Windows smoke test run, rolled back and deleted when the run ends.'" > /dev/null ||
    { say "The checkpoint failed"; exit 1; }
  px "qm listsnapshot $VMID" | grep -q -- "-> $SNAPSHOT " || { say "The checkpoint isn't listed"; exit 1; }
}
restore_checkpoint() {
  [[ -n "$SNAPSHOT" ]] || return 0
  say "Rolling VM $VMID back to $SNAPSHOT and deleting it"
  if ! rollback_to "$SNAPSHOT"; then
    # The checkpoint is the only way back: it stays, and the run fails.
    say "The rollback to $SNAPSHOT failed, so VM $VMID may still hold this run's changes (Smart App Control off, SuperEditr installed). The checkpoint stays: roll back to it by hand, then delete it."
    exit 1
  fi
  if px "qm delsnapshot $VMID $SNAPSHOT" > /dev/null && ! px "qm listsnapshot $VMID" | grep -q -- "-> $SNAPSHOT "; then
    SNAPSHOT=""
  else
    say "Couldn't delete the checkpoint $SNAPSHOT"
    exit 1
  fi
}
# After the run, back to the snapshot it started from. A failed rollback fails the run.
restore_snapshot() {
  [[ "$ROLLED_BACK" == 1 ]] || return 0
  ROLLED_BACK=0
  say "Rolling VM $VMID back to $FROM"
  if ! rollback_to "$FROM"; then
    say "The rollback to $FROM failed, so VM $VMID may still hold this run's changes (SuperEditr installed). Roll it back to $FROM by hand before the next run."
    exit 1
  fi
}
trap 'restore_checkpoint; restore_snapshot' EXIT

# Run -------------------------------------------------------------------------------------------

[[ -n "$FROM" ]] && from_snapshot
[[ "$CHECKPOINT" == 1 ]] && take_checkpoint
[[ "$PREPARE" == 1 && -n "$PROXMOX" && -n "$VMID" ]] && check_free

say "Copying the tests${INSTALLER:+ and the installer}"
vm "Remove-Item -Recurse -Force $REMOTE -ErrorAction SilentlyContinue; New-Item -ItemType Directory -Force $REMOTE\\out | Out-Null" > /dev/null
copy_to "$ROOT/tests/ui/windows/"{driver.ps1,admin.ps1,helpers.ps1,desktop.cs,fake-claude.cs,racepad.cs} "$ROOT/tests/ui/linux/fields.html" \
  "$ROOT/tests/ui/composers/composers.bundle.js" "$VM:C:/SmokeWin/" || { say "Couldn't copy the tests"; exit 1; }
if [[ "$PREPARE" == 1 ]]; then
  SAC_OFF=1 ONLY=warm-up
else
  copy_to "$INSTALLER" "$VM:C:/SmokeWin/setup.exe" || { say "Couldn't copy the installer"; exit 1; }
fi
# admin_ok ACTION...: runs a step that needs an administrator and says what it printed. If the step
# failed, the run ends there, so no test runs on a machine left half prepared.
admin_ok() {
  local out
  out="$(admin "$@")" || { say "The test machine's $1 step failed: $out"; exit 1; }
  say "$out"
}
# A snapshot comes back with the clock it had when it was taken.
[[ -n "$FROM" ]] && admin_ok clock -Seconds "$(date +%s)"
[[ "$SAC_OFF" == 1 ]] && admin_ok smart-app-control-off
[[ "$PREPARE" == 1 ]] && admin_ok helpers "$REMOTE"
if [[ -n "$SCALE" ]]; then
  # The machine restarts as this step ends, which can cut the connection, so its own words say
  # whether it worked; the restart is confirmed by the boot time, and the scale is read back once
  # the desktop user is signed in again, which is when Windows applies it.
  booted="$(admin boot-time)"
  scaled="$(admin scale "$REMOTE" "$USER_NAME" "$SCALE")"
  [[ "$scaled" == *"scale $SCALE% for"* ]] || { say "Couldn't set the display scale: $scaled"; exit 1; }
  say "$scaled"
  sleep 30
  # Back once SSH answers and the desktop user is signed in again.
  for _ in $(seq 60); do
    [[ "$(admin session "$REMOTE" "$USER_NAME" 2> /dev/null)" == yes ]] && break
    sleep 5
  done
  sleep 20
  [[ -n "$booted" && "$(admin boot-time)" != "$booted" ]] || { say "The test machine didn't restart after the scale change"; exit 1; }
  now="$(admin scale-is "$REMOTE" "$USER_NAME")"
  [[ "$now" == "$SCALE" ]] || { say "The display scale is ${now:-unknown}%, not $SCALE%"; exit 1; }
fi
session="$(admin session "$REMOTE" "$USER_NAME")"
[[ "$session" == yes ]] || { say "$USER_NAME isn't signed in at the test machine's desktop: $session"; exit 1; }

say "Running the tests in $USER_NAME's session"
# Cases joined with commas: the test machine's shell would split them at spaces.
admin start "$REMOTE" "$USER_NAME" ${ONLY:+"'${ONLY// /,}'"} > /dev/null
mkdir -p "$OUT/shots"
# The virtual machine's console, as anyone in front of it would see it: the lock screen included.
console_shot() {
  [[ -n "$PROXMOX" && -n "$VMID" ]] || return 0
  local tmp="/tmp/smoke-console-$VMID.ppm"
  px "qm monitor $VMID <<< 'screendump $tmp' > /dev/null" || return 0
  if copy_to "$PROXMOX:$tmp" "$OUT/shots/$1.ppm"; then
    command -v sips > /dev/null && sips -s format png "$OUT/shots/$1.ppm" --out "$OUT/shots/$1.png" > /dev/null && rm -f "$OUT/shots/$1.ppm"
  fi
  px "rm -f $tmp"
}
# Does what the driver asked for, and answers it.
answer() {
  local done=""
  case "$1" in
    lock)
      if [[ -n "$PROXMOX" && -n "$VMID" ]]; then
        console_shot console-lock-screen
        px "qm sendkey $VMID ctrl-shift-f2" && sleep 3
        console_shot console-lock-screen-after-the-shortcut
        done="pressed the review shortcut at the lock screen; "
      fi
      done+="$(admin unlock "$REMOTE" "$USER_NAME"); unlocked"
      ;;
    *) done="nothing to do for '$1'" ;;
  esac
  vm "Set-Content -Path $REMOTE\\out\\answer.txt -Value '${done//\'/}'" > /dev/null
  say "Asked to $1: $done"
}
state=""
for _ in $(seq 480); do
  sleep 5
  state="$(admin state "$REMOTE")"
  if [[ "$state" == asking* ]]; then
    answer "${state#asking }"
    continue
  fi
  [[ "$state" == running ]] || break
done
say "The driver: $state"
admin stop > /dev/null

copy_to -r "$VM:C:/SmokeWin/out" "$OUT/vm" || say "Couldn't fetch the results"
for f in results.tsv summary.txt capture.log; do [[ -f "$OUT/vm/$f" ]] && mv "$OUT/vm/$f" "$OUT/"; done
# Next to the console's photographs, if any.
[[ -d "$OUT/vm/shots" ]] && find "$OUT/vm/shots" -type f -exec mv {} "$OUT/shots/" \;
rm -rf "$OUT/vm"
vm "Remove-Item -Recurse -Force $REMOTE -ErrorAction SilentlyContinue" > /dev/null

[[ -f "$OUT/summary.txt" ]] && cat "$OUT/summary.txt"
[[ "$state" == "done" && -f "$OUT/results.tsv" ]] || { say "The driver didn't finish"; exit 1; }
! cut -f2 "$OUT/results.tsv" | grep -q FAIL
