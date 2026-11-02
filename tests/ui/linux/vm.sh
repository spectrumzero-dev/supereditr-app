# shellcheck shell=bash
# The Linux smoke tests' helpers, sourced on the test machine for every step that
# scripts/ui-smoke-linux.sh runs there. DESKTOP picks the desktop layer (hyprland.sh).

# The desktop session's environment, as SuperEditr and the apps under test see it.
set -a
eval "$(systemctl --user show-environment | grep -E '^(PATH|XDG_[A-Z_]+|WAYLAND_DISPLAY|DISPLAY|HYPRLAND_INSTANCE_SIGNATURE|DBUS_SESSION_BUS_ADDRESS|LANG|OMARCHY_PATH)=')"
set +a
SMOKE="$HOME/supereditr-smoke"
OUT="$SMOKE/out"
LOG="${XDG_STATE_HOME:-$HOME/.local/state}/supereditr/capture.log"
# shellcheck disable=SC2034 # used by commands the controller sends
SETTINGS="$HOME/.config/app.supereditr/settings.json"
CDP_PORT=9222
mkdir -p "$OUT"
# shellcheck source=hyprland.sh
source "$SMOKE/${DESKTOP:-hyprland}.sh"

now_ms() { date +%s%3N; }

# A screenshot of every output, kept as an artifact.
shot() { timeout 10 grim "$OUT/$1.png" 2> /dev/null || echo "no screenshot $1" >&2; }

# capture.log's length, to read only what a step adds.
log_mark() { if [ -f "$LOG" ]; then wc -l < "$LOG"; else echo 0; fi; }
log_since() { [ -f "$LOG" ] && tail -n +"$(($1 + 1))" "$LOG"; }

# wait_log MARK REGEX SECONDS prints the first line after MARK that matches, or nothing.
wait_log() {
  local end=$(($(now_ms) + $3 * 1000)) line
  while (($(now_ms) < end)); do
    line="$(log_since "$1" | grep -E -m1 "$2")" && { echo "$line"; return 0; }
    sleep 0.05
  done
  return 1
}

# Each type on the clipboard with a digest of its bytes: what a restore must give back. Once per
# type: Chromium can offer the same type twice.
clip_digest() {
  local type
  timeout 2 wl-paste --list-types 2> /dev/null | sort -u | while read -r type; do
    printf '%s %s\n' "$type" "$(timeout 2 wl-paste --no-newline --type "$type" 2> /dev/null | md5sum | cut -c1-12)"
  done
}

clip_text() { timeout 2 wl-paste --no-newline --type text/plain 2> /dev/null; }

panel_text() { timeout 10 python3 "$SMOKE/probe.py" panel | jq -r .text; }

# wait_panel SECONDS: milliseconds until the panel is shown with keyboard focus, or "none".
wait_panel() {
  local start end
  start=$(now_ms) end=$(($(now_ms) + $1 * 1000))
  while (($(now_ms) < end)); do
    [ "$(d_panel | jq -r '.shown and .focused')" = true ] && { echo $(($(now_ms) - start)); return 0; }
    sleep 0.03
  done
  echo none
  return 1
}

# wait_panel_text REGEX SECONDS prints the panel's text once it matches.
wait_panel_text() {
  local end=$(($(now_ms) + $2 * 1000)) text
  while (($(now_ms) < end)); do
    text="$(panel_text)"
    grep -qE "$1" <<< "$text" && { echo "$text"; return 0; }
    sleep 0.2
  done
  echo "$text"
  return 1
}

# wait_closed SECONDS: whether the panel closes in time.
wait_closed() {
  local end=$(($(now_ms) + $1 * 1000))
  while (($(now_ms) < end)); do
    [ "$(d_panel | jq -r .shown)" = false ] && return 0
    sleep 0.05
  done
  return 1
}

# The focused field in the app with this pid, through AT-SPI.
field() { timeout 10 python3 "$SMOKE/probe.py" focused "$1"; }

# JavaScript in the test page, through Chromium's DevTools port.
cdp() { timeout 15 python3 "$SMOKE/cdp.py" "$CDP_PORT" fields.html "$1"; }

# Focuses a field on the test page and selects all of its text.
cdp_select() {
  cdp "(() => { const f = document.getElementById('$1'); f.scrollIntoView({block: 'center'}); f.focus();
    if ('select' in f && f.tagName !== 'DIV') f.select();
    else { const r = document.createRange(); r.selectNodeContents(f); const s = getSelection(); s.removeAllRanges(); s.addRange(r); }
    return document.activeElement === f || f.contains(document.activeElement); })()"
}

# A field's text: a text box's value, or an editable element's visible text.
cdp_value() { cdp "(() => { const f = document.getElementById('$1'); return 'value' in f && f.tagName !== 'DIV' ? f.value : f.innerText; })()"; }

wait_for_window() {
  local end=$(($(now_ms) + $2 * 1000)) addr
  while (($(now_ms) < end)); do
    addr="$(d_addr "$1")"
    [ "$addr" != null ] && [ -n "$addr" ] && { echo "$addr"; return 0; }
    sleep 0.2
  done
  echo null
  return 1
}

# Quits gnome-text-editor. Its process name is cut to 15 characters, as Linux does with every one.
editor_quit() {
  pkill -x gnome-text-edit || true
  for _ in $(seq 30); do pgrep -x gnome-text-edit > /dev/null || return 0; sleep 0.1; done
  pkill -KILL -x gnome-text-edit || true
}

# select_all ADDRESS: selects all of the focused field's text in that window with Ctrl+A, and
# waits until AT-SPI reports the selection, so a review never starts before it's there.
select_all() {
  local pid
  pid="$(d_pid "$1")"
  for _ in 1 2 3 4 5; do
    d_focus "$1"
    sleep 0.2
    d_keys "$1" CTRL A
    sleep 0.4
    field "$pid" | jq -e '.chars > 0 and .selection[0] == 0 and .selection[1] == .chars' > /dev/null && return 0
    sleep 0.6
  done
  echo "couldn't select all in $1" >&2
  return 1
}

app_running() { pgrep -x supereditr > /dev/null; }
app_start() { d_exec supereditr; }
app_quit() {
  pkill -x supereditr || true
  for _ in $(seq 50); do app_running || return 0; sleep 0.1; done
  pkill -KILL -x supereditr || true
}
