#!/usr/bin/env bash
# Runs the Linux UI smoke tests against SuperEditr installed on a desktop virtual machine on a
# Proxmox host. It runs on another computer (the controller), which reaches both over SSH:
#
#   - the virtual machine, as the desktop's user, to start apps, read the screen's windows, the
#     accessibility bus and the clipboard, and take grim screenshots (tests/ui/linux);
#   - the Proxmox host, to press real keys on the virtual machine's keyboard (`qm sendkey`) and its
#     pointer's buttons (`qm monitor`). Keys meant as the user's own (the review shortcut, Alt+Enter,
#     Esc) take this path, so they go through the desktop's binds as a person's keys do.
#
# Never wtype: after a wtype keystroke, Hyprland's send_shortcut fails until a real key is pressed.
#
#   scripts/ui-smoke-linux.sh --vm USER@HOST --proxmox USER@HOST --vmid ID --package URL [options]
#
#   --desktop NAME     the desktop layer, tests/ui/linux/NAME.sh (default hyprland)
#   --snapshot NAME    roll the virtual machine back to this snapshot first, and start it
#   --deps "URL ..."   more packages to install with SuperEditr (gnome-text-editor, fcitx5-hangul)
#   --identity FILE    the SSH key for both hosts
#   --out DIR          where results and screenshots go (default build/ui-smoke-linux)
#   --only "CASE ..."  run only these cases (setup always runs)
#   --keep-install     skip installing and the first launch: test the SuperEditr already set up there
#   --hard             also run the hard cases (more monitors, scaling, XWayland, an input method,
#                      a large field, a frozen app, a silent program on the accessibility bus,
#                      Hyprland restarting)
#
# Exit status: 0 when every check passed, 1 otherwise. OUT/summary.txt lists every check with its
# time, OUT/results.tsv has one line per check, and OUT/shots holds the screenshots.
# Not -e: one failed step is a failed check, and the run goes on to the next.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

DESKTOP=hyprland VM="" PROXMOX="" VMID="" PACKAGE="" SNAPSHOT="" DEPS="" IDENTITY="" ONLY="" HARD=0 KEEP=0
OUT="$ROOT/build/ui-smoke-linux"
while (($#)); do
  case "$1" in
    --desktop) DESKTOP="$2"; shift ;;
    --vm) VM="$2"; shift ;;
    --proxmox) PROXMOX="$2"; shift ;;
    --vmid) VMID="$2"; shift ;;
    --package) PACKAGE="$2"; shift ;;
    --snapshot) SNAPSHOT="$2"; shift ;;
    --deps) DEPS="$2"; shift ;;
    --identity) IDENTITY="$2"; shift ;;
    --out) OUT="$2"; shift ;;
    --only) ONLY="$2"; shift ;;
    --hard) HARD=1 ;;
    --keep-install) KEEP=1 ;;
    *) sed -n '2,26p' "$0" >&2; exit 2 ;;
  esac
  shift
done
[[ -n "$VM" && -n "$PROXMOX" && -n "$VMID" && -n "$PACKAGE" ]] || { sed -n '2,26p' "$0" >&2; exit 2; }
[[ -f "$ROOT/tests/ui/linux/$DESKTOP.sh" ]] || { echo "No desktop layer tests/ui/linux/$DESKTOP.sh" >&2; exit 2; }

mkdir -p "$OUT"
rm -rf "${OUT:?}/shots" "$OUT/results.tsv" "$OUT/summary.txt"
# A short path: an SSH control socket's path must fit in 104 bytes.
CONTROL="$(mktemp -d /tmp/ui-smoke.XXXXXX)"
SSH=(ssh -o ControlMaster=auto -o "ControlPath=$CONTROL/%C" -o ControlPersist=300 -o ConnectTimeout=10
  -o ServerAliveInterval=10 -o StrictHostKeyChecking=accept-new -o LogLevel=ERROR)
[[ -n "$IDENTITY" ]] && SSH+=(-i "$IDENTITY" -o IdentitiesOnly=yes)
cleanup() {
  for host in "$VM" "$PROXMOX"; do "${SSH[@]}" -O exit "$host" 2> /dev/null || true; done
  rm -rf "$CONTROL"
}
trap cleanup EXIT

# vm 'commands' runs on the virtual machine with tests/ui/linux/vm.sh loaded.
vm() { "${SSH[@]}" "$VM" "DESKTOP=$DESKTOP; source ~/supereditr-smoke/vm.sh; $1"; }
px() { "${SSH[@]}" "$PROXMOX" "$@"; }
# key COMBO...: real key presses, such as key meta_l-alt-e alt-ret.
key() {
  local cmd="" k
  for k; do cmd+="qm sendkey $VMID $k && "; done
  px "${cmd}true"
}
# click X Y: moves the pointer through the desktop, then presses the left button. QEMU's tablet
# doesn't move the pointer on its own.
click() {
  vm "d_cursor_move $1 $2"
  px "printf 'mouse_button 1\nmouse_button 0\n' | qm monitor $VMID > /dev/null 2>&1"
}

# Results ------------------------------------------------------------------------------------

CASE="" CASE_STARTED=0 PASSED=0 FAILED=0
now_ms() { python3 -c 'import time; print(int(time.time() * 1000))'; }
log() { printf '%s\n' "$@" | tee -a "$OUT/summary.txt"; }
begin_case() {
  CASE="$1" CASE_STARTED=$(now_ms)
  log "" "== $1: $2"
}
end_case() { log "   ($CASE took $((($(now_ms) - CASE_STARTED) / 1000)) s)"; }
# check NAME OK DETAIL
check() {
  local status=PASS
  if [[ "$2" == 1 || "$2" == true ]]; then PASSED=$((PASSED + 1)); else status=FAIL FAILED=$((FAILED + 1)); fi
  log "   $status $1${3:+ — $3}"
  printf '%s\t%s\t%s\t%s\t%s\n' "$CASE" "$status" "$1" "$(($(now_ms) - CASE_STARTED))" "${3:-}" >> "$OUT/results.tsv"
}
note() { log "   NOTE $*"; }
wants() { [[ -z "$ONLY" || " $ONLY " == *" $1 "* ]]; }
# ok COMMAND...: 1 when the command succeeds, 0 otherwise, for check.
ok() { if "$@"; then echo 1; else echo 0; fi; }
has() { grep -qE -- "$2" <<< "$1"; }
lacks() { ! has "$@"; }
is() { [[ "$1" == "$2" ]]; }
# both CMD1 A1 B1 CMD2 A2 B2: two predicates of two arguments each.
both() { "$1" "$2" "$3" && "$4" "$5" "$6"; }
isnt() { [[ "$1" != "$2" ]]; }
below() { (("$1" < "$2")); }

# Whether the panel's card sits wholly on one monitor and touches the target window. The window
# is larger than the card by transparent room for its shadow (src-tauri/src/overlay.rs): 26 pixels
# each side, 16 above and 38 below.
# placement PANEL_JSON WINDOW_JSON prints "ok …" or "bad …".
placement() {
  vm "d_monitors" | jq -r --argjson p "$1" --argjson w "$2" '
    ($p.at[0] + 26) as $x | ($p.at[1] + 16) as $y | ($p.size[0] - 52) as $pw | ($p.size[1] - 54) as $ph
    | [.[] | select($x >= .x and $y >= .y and $x + $pw <= .x + .width and $y + $ph <= .y + .height)] as $on
    | ($x < $w.at[0] + $w.size[0] and $x + $pw > $w.at[0] and $y < $w.at[1] + $w.size[1] and $y + $ph > $w.at[1]) as $touches
    | (if ($on | length) > 0 and $touches then "ok" else "bad" end)
      + " card \($pw)x\($ph) at \($x),\($y) on \(if ($on | length) > 0 then $on[0].name else "no single monitor" end); window at \($w.at[0]),\($w.at[1]) \($w.size[0])x\($w.size[1])"'
}

# Whether the card is next to where it should open, whichever rule applies (capture.rs, anchor):
# the selection if the app reports a position inside its window, else the pointer if it's over the
# window, else near the window's top-left corner. A Wayland app's position is relative to its
# window, and only an XWayland app's is on screen. "Next to" allows for the card's gap, for a line
# of text and for being kept on screen.
# next_to PANEL_JSON WINDOW_JSON RECT_JSON_OR_null POINTER_JSON prints "ok …" or "bad …".
next_to() {
  jq -nr --argjson p "$1" --argjson w "$2" --argjson rect "$3" --argjson pointer "$4" '
    def inside($x; $y): $x >= $w.at[0] and $x < $w.at[0] + $w.size[0] and $y >= $w.at[1] and $y < $w.at[1] + $w.size[1];
    def max2($a; $b): if $a > $b then $a else $b end;
    (if $rect == null then null elif $w.xwayland then $rect else [$rect[0] + $w.at[0], $rect[1] + $w.at[1], $rect[2], $rect[3]] end) as $selection
    | (if $selection != null and inside($selection[0]; $selection[1]) then {kind: "selection", at: $selection}
       elif inside($pointer[0]; $pointer[1]) then {kind: "pointer", at: [$pointer[0] - 24, $pointer[1] - 10, 1, 20]}
       else {kind: "window corner", at: [$w.at[0] + 24, $w.at[1] + 48, 1, 20]} end) as $want
    | ($p.at[0] + 26) as $x | ($p.at[1] + 16) as $y | ($p.size[0] - 52) as $pw | ($p.size[1] - 54) as $ph
    | max2(max2($x - ($want.at[0] + $want.at[2]); $want.at[0] - ($x + $pw)); 0) as $across
    | max2(max2($y - ($want.at[1] + $want.at[3]); $want.at[1] - ($y + $ph)); 0) as $down
    | (if $across <= 40 and $down <= 48 then "ok" else "bad" end)
      + " card \($pw)x\($ph) at \($x),\($y); \($want.kind) at \($want.at | map(floor) | join(",")): \($across) across, \($down) down"'
}

# Steps --------------------------------------------------------------------------------------

boot() {
  begin_case boot "roll back to $SNAPSHOT and start"
  local status
  status="$(px "qm status $VMID" | awk '{print $2}')"
  if [[ "$status" == running ]]; then
    # Omarchy's session ignores the ACPI power button, so shut down from inside.
    vm "sudo systemctl poweroff" > /dev/null 2>&1 || true
    for _ in $(seq 60); do [[ "$(px "qm status $VMID" | awk '{print $2}')" == stopped ]] && break; sleep 2; done
    [[ "$(px "qm status $VMID" | awk '{print $2}')" == stopped ]] || px "qm stop $VMID"
  fi
  "${SSH[@]}" -O exit "$VM" 2> /dev/null || true
  px "qm rollback $VMID $SNAPSHOT && qm start $VMID" > /dev/null
  local up=0
  for _ in $(seq 90); do
    if "${SSH[@]}" -o ConnectTimeout=3 "$VM" 'systemctl --user show-environment | grep -q HYPRLAND_INSTANCE_SIGNATURE && hyprctl -i 0 monitors > /dev/null 2>&1' 2> /dev/null; then up=1; break; fi
    sleep 2
  done
  check "the virtual machine is back at $SNAPSHOT with the desktop running" "$up"
  [[ "$up" == 1 ]] || exit 1
  sleep 5  # the bar and portals finish starting
  end_case
}

install() {
  begin_case install "copy the tests and install SuperEditr"
  COPYFILE_DISABLE=1 tar --no-xattrs -C "$ROOT/tests/ui/linux" -cf - . -C "$ROOT/tests/ui/composers" composers.bundle.js |
    "${SSH[@]}" "$VM" 'rm -rf ~/supereditr-smoke && mkdir -p ~/supereditr-smoke && tar -C ~/supereditr-smoke -xf -'
  # Downloaded first: pacman -U of an address also asks for a signature file, which a private
  # repository may not have.
  vm "app_quit; rm -rf ~/.config/app.supereditr ~/.local/state/supereditr /tmp/smoke-pkgs; mkdir -p /tmp/smoke-pkgs
    (cd /tmp/smoke-pkgs && for url in $PACKAGE $DEPS; do curl -fsSO \"\$url\" || echo \"couldn't download \$url\"; done
     sudo pacman -U --noconfirm ./*.pkg.tar.zst) > \$OUT/pacman.log 2>&1; echo \$?" | {
    read -r status
    check "pacman installs the package" "$(ok is "$status" 0)" "$(basename "$PACKAGE")"
  }
  note "installed: $(vm 'pacman -Q supereditr-bin gnome-text-editor fcitx5-hangul 2> /dev/null | tr "\n" " "')"
  note "desktop: $(vm 'd_name'), $(vm 'd_monitors')"
  end_case
}

first_run() {
  begin_case first-run "first launch, Set up Omarchy, and the next start"
  vm 'app_start'
  local settings
  settings="$(vm 'wait_for_window "^supereditr \| SuperEditr Settings$" 15')"
  check "the first launch opens Settings" "$(ok isnt "$settings" null)"
  local binds
  binds="$(vm 'for _ in $(seq 50); do [ "$(d_binds)" -ge 2 ] && break; sleep 0.2; done; d_binds')"
  check "SuperEditr adds its two binds" "$(ok is "$binds" 2)" "$binds binds"
  vm 'sleep 2; shot 01-first-run'
  # Set up Omarchy, as the setup page's buttons do it, once the window is on the accessibility bus.
  vm 'for _ in $(seq 30); do python3 $SMOKE/probe.py press "SuperEditr Settings" "Set up Omarchy…" | grep -q pressed && break; sleep 0.5; done' > /dev/null
  sleep 1
  local made
  made="$(vm 'python3 $SMOKE/probe.py press "SuperEditr Settings" "Make these changes"; sleep 2; shot 02-set-up-omarchy;
    grep -c -- --force-renderer-accessibility ~/.config/chromium-flags.conf; test -f ~/.config/autostart/supereditr.desktop && echo autostart')"
  check "Set up Omarchy adds the Chromium flag and open at login" "$(ok both has "$made" autostart has "$made" "^[1-9]")" "$(tr '\n' ' ' <<< "$made")"
  local vs_code_flag
  vs_code_flag="$(vm 'grep -c -- --force-renderer-accessibility ~/.config/code-flags.conf')"
  check "Set up Omarchy adds the VS Code flag" "$(ok is "$vs_code_flag" 1)" "$vs_code_flag"
  # The tests need foot reviewed (copy-only) rather than skipped as an excluded app.
  vm 'app_quit; jq ".excluded_apps -= [\"foot\"]" "$SETTINGS" > "$SETTINGS.new" && mv "$SETTINGS.new" "$SETTINGS"'
  vm 'app_start'
  local opened
  opened="$(vm 'for _ in $(seq 40); do [ "$(d_binds)" -ge 2 ] && break; sleep 0.2; done; sleep 3; d_addr "^supereditr \| SuperEditr Settings$"')"
  check "once Omarchy is set up, the next start doesn't open Settings" "$(ok is "$opened" null)"
  [[ "$opened" == null ]] || vm "d_focus $opened; hyprctl dispatch 'hl.dsp.window.close({ window = \"address:$opened\" })' > /dev/null"
  end_case
}

# The DevTools port of the Chromium the test runs now.
CDP=9222
start_chromium() {
  # $1 port, $2 profile name, $3 extra flags, $4 binary
  CDP="$1"
  vm "pkill -x chromium; sleep 1; d_exec \"${4:-chromium} $3 --remote-debugging-port=$1 --user-data-dir=\$SMOKE/$2 --no-first-run --no-default-browser-check --password-store=basic file://\$SMOKE/fields.html\""
  vm "for _ in \$(seq 60); do CDP_PORT=$1 cdp 'document.readyState' 2> /dev/null | grep -q complete && break; sleep 0.5; done; wait_for_window '^chromium(-browser)? \\| SuperEditr smoke fields' 10 > /dev/null; sleep 3"
}

# The user's clipboard before a replacement: "html" (a Chromium copy, about nine types, with the
# test page open), "image" (a PNG) or "text".
set_clipboard() {
  case "$1" in
    html)
      # A popup from the test page, so it opens in the Chromium already running (with CDP_PORT).
      vm "CDP_PORT=$CDP cdp \"window.open('clipboard.html', 'clip', 'popup,width=500,height=200') !== null\" > /dev/null
        c=\$(wait_for_window \"Rhyan's clipboard\" 10); d_focus \$c; sleep 0.5; d_keys \$c CTRL A; sleep 0.2; d_keys \$c CTRL C; sleep 0.5
        hyprctl dispatch \"hl.dsp.window.close({ window = \\\"address:\$c\\\" })\" > /dev/null; sleep 0.5" ;;
    # wl-copy stays behind to serve the copy: without its output redirected, SSH waits for it.
    image) vm 'wl-copy --type image/png < /usr/share/icons/hicolor/128x128/apps/supereditr.png > /dev/null 2>&1; sleep 0.3' ;;
    text) vm "wl-copy 'Rhyan wrote this himself.' < /dev/null > /dev/null 2>&1; sleep 0.3" ;;
  esac
}

# review_and_apply CASE_LABEL WINDOW_REGEX APP_ID SHOT [apply|copy]: presses the review shortcut over
# the selection in the window in front, waits for a suggestion and presses Alt+Enter. Sets
# REVIEW_PANEL, REVIEW_READ and REVIEW_RESULT.
review_and_apply() {
  local label="$1" window="$2" app="$3" name="$4" mode="${5:-apply}" mark win panel_ms text where rect pointer
  mark="$(vm log_mark)"
  win="$(vm "d_active")"
  rect="$(vm "field $(jq -r .pid <<< "$win") | jq -c '.selection_rect // null'")"
  pointer="$(vm d_cursor)"
  key meta_l-alt-e
  panel_ms="$(vm 'wait_panel 6')"
  check "$label: the review shortcut opens the panel with focus" "$(ok isnt "$panel_ms" none)" "after $panel_ms ms (including the key press)"
  REVIEW_READ="$(vm "wait_log $mark 'shortcut $app: ' 3")"
  if [[ "$panel_ms" == none ]]; then REVIEW_RESULT=""; return; fi
  text="$(vm "wait_panel_text '[0-9]+ changes?' 15")"
  check "$label: the panel shows a fix" "$(ok has "$text" "[0-9]+ changes?")" "${text:0:120}"
  REVIEW_PANEL="$(vm panel_placed)"
  where="$(placement "$REVIEW_PANEL" "$win")"
  check "$label: the panel is on one monitor, over the app" "$(ok has "$where" "^ok")" "${where#* }"
  where="$(next_to "$REVIEW_PANEL" "$win" "${rect:-null}" "$pointer")"
  check "$label: the panel is next to what was reviewed" "$(ok has "$where" "^ok")" "${where#* }"
  vm "shot $name-review"
  [[ "$window" == "-" ]] || true
  key alt-ret
  REVIEW_RESULT="$(vm "wait_log $mark 'replace $app: ' 6")"
  if [[ "$mode" == copy ]]; then REVIEW_RESULT="copy"; fi
  vm 'wait_closed 3' || true
  sleep 0.5
}

# clip_same BEFORE AFTER LABEL: the same types with the same bytes. The restore may add the
# password-manager hint, so clipboard history doesn't record the user's own copy a second time.
clip_same() {
  local diff
  diff="$(diff <(echo "$1") <(grep -v '^x-kde-passwordManagerHint ' <<< "$2") | grep '^[<>]' | head -4 | tr '\n' ' ')"
  check "$3" "$(ok both is "$diff" "" isnt "$1" "")" "$(wc -l <<< "$1" | tr -d ' ') types${diff:+; differs: $diff}"
}

chromium_cases() {
  begin_case chromium "the composers page in Chromium, read through AT-SPI and pasted"
  start_chromium 9222 chrome --force-renderer-accessibility
  local page
  page="$(vm "d_addr '^chromium(-browser)? \\| SuperEditr smoke fields'")"
  vm "shot 10-chromium-page"
  local kinds=(html image text) i=0 id before after value
  for id in plain rich code markdown tiptap lexical; do
    set_clipboard "${kinds[$((i % 3))]}"
    i=$((i + 1))
    vm "d_focus $page; sleep 0.3; d_cursor_move \$(d_active | jq -r '.at[0] + 300') \$(d_active | jq -r '.at[1] + 200')" > /dev/null
    local selected
    selected="$(vm "cdp_select $id")"
    [[ "$selected" == true ]] || note "$id: couldn't select the field ($selected)"
    sleep 0.4
    before="$(vm clip_digest)"
    review_and_apply "$id" page chromium "11-chromium-$id"
    check "$id: read through AT-SPI, pasting allowed" "$(ok has "$REVIEW_READ" "via AT-SPI.*pasting allowed")" "${REVIEW_READ#* }"
    check "$id: Alt+Enter applies" "$(ok has "$REVIEW_RESULT" ": applied")" "${REVIEW_RESULT#* }"
    value="$(vm "cdp_value $id")"
    case "$id" in
      plain | rich) check "$id: the field holds the fix" "$(ok both has "$value" "They're going" lacks "$value" "Their going")" "${value:0:100}" ;;
      *)
        check "$id: both fixes land" "$(ok both has "$value" "They're going to love" has "$value" "really run")" "$(tr '\n' '/' <<< "${value:0:140}")"
        local state
        state="$(vm "cdp \"document.getElementById('$id-state').textContent\"")"
        note "$id: the page describes the field as $state"
        ;;
    esac
    after="$(vm 'sleep 0.3; clip_digest')"
    clip_same "$before" "$after" "$id: the clipboard (${kinds[$(((i - 1) % 3))]}) is restored, every type"
    vm "shot 11-chromium-$id-applied"
  done

  # Refused: a password field, and a selection holding a key.
  local mark text
  for id in password secret; do
    vm "d_focus $page; sleep 0.2; cdp_select $id" > /dev/null
    set_clipboard text
    vm "d_focus $page; sleep 0.3; cdp_select $id" > /dev/null
    before="$(vm clip_digest)"
    mark="$(vm log_mark)"
    key meta_l-alt-e
    sleep 1.5
    text="$(vm panel_text)"
    vm "shot 12-chromium-$id"
    if [[ "$id" == password ]]; then
      check "password: the review is refused" "$(ok both lacks "$text" "[0-9] changes?" has "$text" "[Pp]assword")" "panel: ${text:0:120}; log: $(vm "log_since $mark | grep 'shortcut chromium' | cut -d' ' -f2- | head -1")"
    else
      check "secret: the review is refused" "$(ok both lacks "$text" "[0-9] changes?" isnt "$text" "")" "panel: ${text:0:140}"
    fi
    key esc
    vm 'wait_closed 3' || true
    after="$(vm clip_digest)"
    clip_same "$before" "$after" "$id: the clipboard is untouched"
  done
  vm 'pkill -x chromium' || true
  end_case
}

chromium_plain_case() {
  begin_case chromium-plain "Chromium without accessibility: a guarded copy, copy-only"
  # The binary itself, which skips ~/.config/chromium-flags.conf and so the accessibility flag.
  start_chromium 9223 chrome-plain "" /usr/lib/chromium/chromium
  local page before during after
  page="$(vm "d_addr '^chromium(-browser)? \\| SuperEditr smoke fields'")"
  set_clipboard html
  vm "d_focus $page; sleep 0.3; CDP_PORT=9223 cdp_select plain" > /dev/null
  before="$(vm clip_digest)"
  # Started as the binary itself, Chromium names its windows chromium-browser.
  review_and_apply "plain Chromium" page chromium-browser 20-chromium-plain copy
  check "plain Chromium: read with a guarded copy, copy-only" "$(ok has "$REVIEW_READ" "guarded copy.*copy-only")" "${REVIEW_READ#* }"
  during="$(vm clip_text)"
  check "plain Chromium: Alt+Enter copies the fix" "$(ok has "$during" "They're going")" "clipboard: ${during:0:80}"
  after="$(vm "CDP_PORT=9223 cdp_value plain")"
  check "plain Chromium: the field is unchanged" "$(ok has "$after" "Their going")" "${after:0:80}"
  note "the clipboard held $(wc -l <<< "$before" | tr -d ' ') types before the review"
  vm 'pkill -x chromium' || true
  end_case
}

foot_case() {
  begin_case foot "foot: a terminal, read with a guarded copy, copy-only"
  vm "pkill -x foot; echo 'Their going to love this, and it work good.' > /tmp/smoke-foot.txt
    d_exec \"foot -T smoke-foot -e sh -c 'cat /tmp/smoke-foot.txt; exec sh'\""
  local f
  f="$(vm 'wait_for_window "^foot \| smoke-foot" 10')"
  # foot's keyboard search selects text: Ctrl+Shift+R, the first word, then Ctrl+W per extra word.
  vm "d_focus $f; sleep 0.5; d_keys $f 'CTRL SHIFT' R; sleep 0.2; for k in T h e i r; do d_keys $f '' \$k; done
    for _ in 1 2 3 4; do d_keys $f CTRL W; done; sleep 0.2; d_keys $f '' Return; sleep 0.3"
  set_clipboard image
  vm "d_focus $f; sleep 0.3"
  local before mark during after
  before="$(vm clip_digest)"
  mark="$(vm log_mark)"
  key meta_l-alt-e
  vm 'wait_panel 6' > /dev/null || true
  local read
  read="$(vm "wait_log $mark 'shortcut foot: ' 3")"
  check "foot: read with a guarded copy (Ctrl+Insert), copy-only" "$(ok has "$read" "guarded copy.*copy-only")" "${read#* }"
  during="$(vm 'sleep 0.5; clip_digest')"
  clip_same "$before" "$during" "foot: the clipboard is restored after the copy, every type"
  local text
  text="$(vm "wait_panel_text '[0-9]+ changes?' 15")"
  check "foot: the panel shows a fix" "$(ok has "$text" "[0-9]+ changes?")" "${text:0:100}"
  vm 'shot 30-foot-review'
  key alt-ret
  vm 'wait_closed 3' || true
  after="$(vm 'sleep 0.3; clip_text')"
  check "foot: Alt+Enter copies the fix, and nothing is typed into the terminal" "$(ok has "$after" "They're going")" "clipboard: ${after:0:80}; $(vm "log_since $mark | grep -c 'replace foot'") replace attempts"
  vm 'shot 31-foot-after; pkill -x foot' || true
  end_case
}

SAMPLE="Their going to love this, and it work good. See you tomorow."

start_editor() {
  # $1 extra environment (GDK_BACKEND=x11), $2 file name
  # Without its saved session, the editor reopens neither earlier files nor their unsaved drafts.
  vm "editor_quit; rm -rf ~/.local/share/org.gnome.TextEditor; printf '%s' \"$SAMPLE\" > /tmp/$2; sleep 0.5
    d_exec '${1:+env $1 }gnome-text-editor --standalone --ignore-session /tmp/$2'
    e=\$(wait_for_window '^(org.gnome.TextEditor|gnome-text-editor) \\| $2' 15); d_focus \$e
    for _ in \$(seq 30); do field \$(d_pid \$e) | jq -e '.chars > 0' > /dev/null && break; sleep 0.3; done; echo \$e"
}

editor_cases() {
  begin_case editor "gnome-text-editor: applied, verified, one undo; quick fix"
  local e pid before after value mark
  e="$(start_editor "" smoke-sample.txt | tail -1)"
  pid="$(vm "d_pid $e")"
  check "gnome-text-editor opens" "$(ok both isnt "$e" null isnt "$pid" "")" "$(vm d_active)"
  set_clipboard image
  vm "d_focus $e; sleep 0.3; select_all $e; sleep 0.3"
  before="$(vm clip_digest)"
  review_and_apply "gnome-text-editor" "$e" org.gnome.TextEditor 40-editor
  check "gnome-text-editor: read through AT-SPI, pasting allowed" "$(ok has "$REVIEW_READ" "via AT-SPI.*pasting allowed")" "${REVIEW_READ#* }"
  check "gnome-text-editor: Alt+Enter applies, verified through AT-SPI" "$(ok has "$REVIEW_RESULT" ": applied")" "${REVIEW_RESULT#* }"
  value="$(vm "field $pid | jq -r .text")"
  check "gnome-text-editor: the document holds the fix" "$(ok both has "$value" "They're going" has "$value" tomorrow)" "${value:0:100}"
  after="$(vm 'sleep 0.3; clip_digest')"
  clip_same "$before" "$after" "gnome-text-editor: the clipboard (image) is restored"
  vm "d_keys $e CTRL Z; sleep 0.5"
  value="$(vm "field $pid | jq -r .text")"
  check "gnome-text-editor: one undo puts the original back" "$(ok is "$value" "$SAMPLE")" "${value:0:100}"
  vm 'shot 41-editor-undone'

  # Quick fix: no panel, the fix goes straight in.
  vm "d_focus $e; sleep 0.3; select_all $e; sleep 0.3"
  mark="$(vm log_mark)"
  key meta_l-ctrl-alt-e
  local result
  result="$(vm "wait_log $mark 'replace org.gnome.TextEditor: ' 10")"
  check "quick fix: applies without a review" "$(ok has "$result" ": applied")" "${result#* }"
  value="$(vm "field $pid | jq -r .text")"
  check "quick fix: the document holds the fix" "$(ok has "$value" "They're going")" "${value:0:100}"
  vm 'shot 42-editor-quick-fix'
  end_case
}

placement_case() {
  begin_case placement "the panel opens by the selection (or the pointer), on screen and over the app"
  local e corner x y panel win where rect
  e="$(vm "d_addr '^org.gnome.TextEditor'")"
  [[ "$e" != null ]] || e="$(start_editor "" smoke-sample.txt | tail -1)"
  vm "d_focus $e; sleep 0.3; d_keys $e CTRL Z; sleep 0.3; select_all $e"
  win="$(vm d_active)"
  for corner in top-left bottom-right; do
    if [[ "$corner" == top-left ]]; then
      x="$(jq -r '.at[0] + 60' <<< "$win")" y="$(jq -r '.at[1] + 80' <<< "$win")"
    else
      x="$(jq -r '.at[0] + .size[0] - 40' <<< "$win")" y="$(jq -r '.at[1] + .size[1] - 40' <<< "$win")"
    fi
    vm "d_cursor_move $x $y"
    rect="$(vm "field $(jq -r .pid <<< "$win") | jq -c '.selection_rect // null'")"
    key meta_l-alt-e
    vm 'wait_panel 6' > /dev/null || true
    sleep 0.5
    panel="$(vm panel_placed)"
    where="$(placement "$panel" "$win")"
    check "pointer at the $corner: the panel is on screen, over the app" "$(ok has "$where" "^ok")" "pointer $x,$y; ${where#* }"
    where="$(next_to "$panel" "$win" "${rect:-null}" "[$x,$y]")"
    check "pointer at the $corner: the panel is next to what was reviewed" "$(ok has "$where" "^ok")" "${where#* }"
    vm "shot 45-placement-$corner"
    key esc
    vm 'wait_closed 3' || true
  done
  # A notice has no selection to go by. It stays at the pointer after the panel takes focus, which
  # can move the pointer. Nothing is selected in a new terminal, so it gets one.
  vm "d_exec \"foot -T smoke-notice\""
  local terminal
  terminal="$(vm 'wait_for_window "^foot \| smoke-notice" 10')"
  win="$(vm "d_focus $terminal; sleep 0.5; d_active")"
  x="$(jq -r '.at[0] + 60' <<< "$win")" y="$(jq -r '.at[1] + 80' <<< "$win")"
  vm "d_cursor_move $x $y"
  key meta_l-alt-e
  vm 'wait_panel 6' > /dev/null || true
  sleep 1
  panel="$(vm panel_placed)"
  where="$(next_to "$panel" "$win" null "[$x,$y]")"
  check "with nothing selected, the notice stays at the pointer" "$(ok has "$where" "^ok")" "${where#* }"
  vm 'shot 45-placement-notice'
  key esc
  vm 'wait_closed 3' || true
  vm "hyprctl dispatch 'hl.dsp.window.close({ window = \"address:$terminal\" })' > /dev/null"
  end_case
}

# Settings' Try it now box, with Settings on the right of the screen: the box's position is inside
# the window, so a panel placed as if it were on screen opens far to the left of it.
try_it_case() {
  begin_case try-it "Try it now's panel opens at its box, with Settings on the right"
  local e win box panel where page
  e="$(vm "d_addr '^org.gnome.TextEditor'")"
  [[ "$e" != null ]] || e="$(start_editor "" smoke-sample.txt | tail -1)"
  vm "d_focus $e; sleep 0.3"
  vm 'app_start'
  vm 'wait_for_window "^supereditr \| SuperEditr Settings$" 10' > /dev/null
  win="$(vm 'sleep 1; d_windows' | jq -c '[.[] | select(.title == "SuperEditr Settings")][0] | {at, size, xwayland}')"
  check "Settings is on the right of the screen" "$(ok below 300 "$(jq -r '.at[0]' <<< "$win")")" "$win"
  page="$(vm 'for _ in $(seq 20); do python3 $SMOKE/probe.py press "SuperEditr Settings" "Review" | tee /tmp/smoke-press | grep -q pressed && break; python3 $SMOKE/probe.py press "SuperEditr Settings" "Get Started" > /dev/null; sleep 0.5; done; cat /tmp/smoke-press; rm -f /tmp/smoke-press')"
  check "Try it now's Review button can be pressed" "$(ok has "$page" pressed)" "$page"
  vm 'wait_panel_text "[0-9]+ changes?" 15' > /dev/null
  panel="$(vm panel_placed)"
  box="$(vm 'python3 $SMOKE/probe.py editable "SuperEditr Settings"' | jq -c '.rect // null')"
  where="$(next_to "$panel" "$win" "$box" "[-1,-1]")"
  check "the panel is next to the Try it now box" "$(ok has "$where" "^ok .*; selection at")" "box $box; ${where#* }"
  vm 'shot 46-try-it'
  key esc
  vm 'wait_closed 3' || true
  vm "a=\$(d_addr '^supereditr \\| SuperEditr Settings\$'); d_focus \$a; hyprctl dispatch \"hl.dsp.window.close({ window = \\\"address:\$a\\\" })\" > /dev/null"
  end_case
}

reload_case() {
  begin_case reload "hyprctl reload, then a restart of SuperEditr: binds come back, once"
  local mark e binds line
  mark="$(vm log_mark)"
  vm 'd_reload'
  line="$(vm "wait_log $mark 'reloaded its config' 5")"
  binds="$(vm 'sleep 1; d_binds')"
  check "after a reload, the binds are back, once" "$(ok is "$binds" 2)" "$binds binds; ${line#* }"
  e="$(vm "d_addr '^org.gnome.TextEditor'")"
  vm "d_focus $e; sleep 0.3; select_all $e"
  key meta_l-alt-e
  check "after a reload, the shortcut opens a review" "$(ok isnt "$(vm 'wait_panel 6')" none)"
  vm 'shot 50-after-reload'
  key esc
  vm 'wait_closed 3' || true

  vm 'app_quit'
  binds="$(vm 'sleep 0.5; d_binds')"
  check "quitting removes the binds" "$(ok is "$binds" 0)" "$binds binds"
  vm 'app_start'
  binds="$(vm 'for _ in $(seq 50); do [ "$(d_binds)" -ge 2 ] && break; sleep 0.2; done; sleep 2; d_binds')"
  check "after a restart, the binds are there once" "$(ok is "$binds" 2)" "$binds binds"
  vm "d_focus $e; sleep 0.3; select_all $e"
  key meta_l-alt-e
  check "after a restart, the shortcut opens a review" "$(ok isnt "$(vm 'wait_panel 6')" none)"
  key esc
  vm 'wait_closed 3' || true
  end_case
}

# Hard cases ---------------------------------------------------------------------------------

source "$ROOT/tests/ui/linux/hard-cases.sh"

# Run ----------------------------------------------------------------------------------------

STARTED=$(now_ms)
log "SuperEditr Linux smoke tests, desktop $DESKTOP, $(date -u +%Y-%m-%dT%H:%MZ)" "package $PACKAGE"
[[ -n "$SNAPSHOT" ]] && boot
if [[ "$KEEP" == 1 ]]; then
  COPYFILE_DISABLE=1 tar --no-xattrs -C "$ROOT/tests/ui/linux" -cf - . -C "$ROOT/tests/ui/composers" composers.bundle.js |
    "${SSH[@]}" "$VM" 'mkdir -p ~/supereditr-smoke && tar -C ~/supereditr-smoke -xf -'
  vm 'app_running || app_start; sleep 2'
else
  install
  first_run
fi
wants chromium && chromium_cases
wants chromium-plain && chromium_plain_case
wants foot && foot_case
wants editor && editor_cases
wants placement && placement_case
wants try-it && try_it_case
wants reload && reload_case
if [[ "$HARD" == 1 ]]; then hard_cases; fi

mkdir -p "$OUT/shots"
"${SSH[@]}" "$VM" 'tar -C ~/supereditr-smoke/out -cf - .' | tar -C "$OUT/shots" -xf -
"${SSH[@]}" "$VM" 'cat ~/.local/state/supereditr/capture.log' > "$OUT/capture.log" 2> /dev/null || true
log "" "$PASSED passed, $FAILED failed in $((($(now_ms) - STARTED) / 1000)) s"
((FAILED == 0))
