# shellcheck shell=bash
# The Linux smoke tests' hard cases, sourced by scripts/ui-smoke-linux.sh and run with --hard. They
# use its helpers (vm, key, check, note, review_and_apply, placement, start_editor, start_chromium).

# A second output, fractional scaling on each, and the panel staying on the app's own monitor.
monitors_case() {
  begin_case monitors "a second monitor at a fractional scale (Hyprland picks the nearest valid one to 1.5), then the first at 1.25"
  vm 'd_output_add SMOKE-2; sleep 1; d_monitor SMOKE-2 1600x900 1280x0 1.5; sleep 1'
  note "monitors: $(vm d_monitors)"
  # A new window opens on the focused monitor.
  vm 'hyprctl dispatch "hl.dsp.focus({ monitor = \"SMOKE-2\" })" > /dev/null; sleep 0.5'
  local e win
  e="$(start_editor "" smoke-monitor.txt | tail -1)"
  win="$(vm d_active)"
  check "the editor opens on the second monitor" "$(ok has "$win" '"monitor":1')" "$win"
  vm "select_all $e; d_cursor_move \$(jq -r '.at[0] + 100' <<< '$win') \$(jq -r '.at[1] + 120' <<< '$win')"
  review_and_apply "fractional scale, second monitor" "$e" org.gnome.TextEditor 60-monitor-second
  check "fractional scale, second monitor: Alt+Enter applies" "$(ok has "$REVIEW_RESULT" ": applied")" "${REVIEW_RESULT#* }"

  vm 'd_monitor Virtual-1 1280x800 0x0 1.25; sleep 1; hyprctl dispatch "hl.dsp.focus({ monitor = \"Virtual-1\" })" > /dev/null; sleep 0.5'
  note "monitors: $(vm d_monitors)"
  e="$(start_editor "" smoke-scaled.txt | tail -1)"
  win="$(vm d_active)"
  vm "select_all $e"
  review_and_apply "scale 1.25, first monitor" "$e" org.gnome.TextEditor 61-monitor-scaled
  check "scale 1.25, first monitor: Alt+Enter applies" "$(ok has "$REVIEW_RESULT" ": applied")" "${REVIEW_RESULT#* }"
  # Back as it was: the reload puts the monitors back as configured (and SuperEditr's binds).
  vm 'editor_quit; d_output_remove SMOKE-2; d_reload; sleep 2'
  check "the monitors are back as they were" "$(ok is "$(vm 'd_monitors | jq -c "[.[] | [.name, .scale]]"')" '[["Virtual-1",1]]')"
  end_case
}

xwayland_case() {
  begin_case xwayland "gnome-text-editor through XWayland"
  local e win pid value
  e="$(start_editor GDK_BACKEND=x11 smoke-x11.txt | tail -1)"
  win="$(vm d_active)"
  pid="$(jq -r .pid <<< "$win")"
  check "the editor runs under XWayland" "$(ok has "$win" '"xwayland":true')" "$win"
  # Under XWayland, GTK 4 gives the text view keyboard focus only once it's clicked, as a person
  # would before selecting; the first click after the window opens only activates it.
  local tries
  for tries in 1 2 3; do
    click "$(jq -r '.at[0] + .size[0] / 2 | floor' <<< "$win")" "$(jq -r '.at[1] + .size[1] / 2 | floor' <<< "$win")"
    sleep 0.4
    vm "field $pid | jq -e .chars" > /dev/null && break
  done
  note "the text view took focus after $tries clicks"
  vm "select_all $e"
  # An XWayland window's class is the program's name.
  review_and_apply "XWayland" "$e" gnome-text-editor 65-xwayland
  check "XWayland: read through AT-SPI, pasting allowed" "$(ok has "$REVIEW_READ" "via AT-SPI.*pasting allowed")" "${REVIEW_READ#* }"
  check "XWayland: Alt+Enter applies" "$(ok has "$REVIEW_RESULT" ": applied")" "${REVIEW_RESULT#* }"
  value="$(vm "field $pid | jq -r .text")"
  check "XWayland: the document holds the fix" "$(ok has "$value" "They're going")" "${value:0:80}"
  vm 'editor_quit'
  end_case
}

# fcitx5 with a Korean input method, composing a syllable when the user acts. The user's own keys
# go through the input method, which commits the syllable first; keys the test sends straight to
# the window (send_shortcut) skip it, so this case uses only real key presses.
fcitx_case() {
  begin_case fcitx5 "an input method composing (fcitx5, Hangul) when the user selects and reviews"
  # fcitx5 saves its profile when it stops: stop it, then write the profile.
  vm 'p=~/.config/fcitx5/profile; systemctl --user stop omarchy-fcitx5; cp $p $p.smoke-bak
    printf "[Groups/0]\nName=Default\nDefault Layout=us\nDefaultIM=hangul\n\n[Groups/0/Items/0]\nName=keyboard-us\nLayout=\n\n[Groups/0/Items/1]\nName=hangul\nLayout=\n\n[GroupOrder]\n0=Default\n" > $p
    systemctl --user start omarchy-fcitx5; sleep 3'
  local e pid value mark panel
  e="$(start_editor "" smoke-ime.txt | tail -1)"
  pid="$(vm "d_pid $e")"
  vm "d_keys $e CTRL End; sleep 0.3; fcitx5-remote -s hangul; sleep 0.3"
  check "fcitx5: the Hangul input method is on" "$(ok is "$(vm 'fcitx5-remote -n')" hangul)"
  # g, k, s on a 2-set Korean keyboard: 한, still being composed.
  key spc g k s
  sleep 0.4
  vm 'shot 70-fcitx5-composing'
  value="$(vm "field $pid | jq -r .text")"
  check "fcitx5: the syllable is still being composed (not in the text yet)" "$(ok lacks "$value" '한')" "${value:0:80}"

  # The shortcut with nothing selected: the input method commits the syllable, nothing is lost.
  mark="$(vm log_mark)"
  key meta_l-alt-e
  vm 'wait_panel 6' > /dev/null
  sleep 1
  note "with nothing selected: $(vm panel_text | cut -c1-100); read: $(vm "log_since $mark | grep 'shortcut org' | cut -d' ' -f2-")"
  key esc
  vm 'wait_closed 3' || true
  value="$(vm "field $pid | jq -r .text")"
  check "fcitx5: the shortcut mid-composition keeps the text and the syllable once" \
    "$(ok both has "$value" "^${SAMPLE//./\\.} " is "$(grep -o '한' <<< "$value" | wc -l | tr -d ' ')" 1)" "${value:0:100}"

  # Composing again, then Ctrl+A and a review.
  key end g k s ctrl-a
  sleep 0.5
  mark="$(vm log_mark)"
  key meta_l-alt-e
  panel="$(vm 'wait_panel 6')"
  vm "wait_panel_text '[0-9]+ changes?' 10" > /dev/null
  vm 'shot 71-fcitx5-review'
  key alt-ret
  vm 'wait_closed 3' || true
  sleep 0.5
  value="$(vm "field $pid | jq -r .text")"
  vm 'shot 72-fcitx5-after'
  check "fcitx5: Ctrl+A and a review over a composition apply" "$(ok isnt "$panel" none)" "$(vm "log_since $mark | grep -E 'shortcut org|replace org' | cut -d' ' -f2- | tr '\n' '|'")"
  check "fcitx5: the fix lands, with both syllables there once each" \
    "$(ok both has "$value" "They're going" is "$(grep -o '한' <<< "$value" | wc -l | tr -d ' ')" 2)" "${value:0:120}"
  vm 'fcitx5-remote -c; p=~/.config/fcitx5/profile; systemctl --user stop omarchy-fcitx5; mv $p.smoke-bak $p; systemctl --user start omarchy-fcitx5; editor_quit'
  end_case
}

large_case() {
  begin_case large "a text box of 20,000 characters (hyprcorrect stops at 8,192)"
  start_chromium 9222 chrome --force-renderer-accessibility
  local page value length
  page="$(vm "d_addr '^chromium(-browser)? \\| SuperEditr smoke fields'")"
  vm "d_focus $page; sleep 0.3; cdp_select large" > /dev/null
  length="$(vm "cdp \"document.getElementById('large').value.length\"")"
  review_and_apply "large" "$page" chromium 75-large
  check "large: read through AT-SPI, pasting allowed" "$(ok has "$REVIEW_READ" "read $length characters via AT-SPI.*pasting allowed")" "${REVIEW_READ#* }"
  check "large: Alt+Enter applies" "$(ok has "$REVIEW_RESULT" ": applied")" "${REVIEW_RESULT#* }"
  value="$(vm "cdp \"(() => { const v = document.getElementById('large').value; return v.length + ' ' + v.slice(-40); })()\"")"
  check "large: the whole box is there, with the fix at its end" "$(ok has "$value" "They're going to love this")" "$length characters before; now ${value}"
  vm 'pkill -x chromium'
  end_case
}

frozen_case() {
  begin_case frozen "the app in front stops answering (SIGSTOP)"
  local e pid mark lines took said
  e="$(start_editor "" smoke-frozen.txt | tail -1)"
  pid="$(vm "d_pid $e")"
  vm "select_all $e; sleep 0.3; kill -STOP $pid"
  mark="$(vm log_mark)"
  key meta_l-alt-e
  # Timed by SuperEditr's own log, from the shortcut reaching it to its panel taking focus: while
  # an app is stopped, the test's own window and accessibility queries can stall.
  vm "wait_log $mark 'panel focused' 8" > /dev/null
  lines="$(vm "log_since $mark")"
  took="$(awk '/shortcut review from/ && !s {s = $1} /panel focused/ && s && !e {e = $1} END {if (s && e) printf "%d", (e - s) * 1000; else print 99999}' <<< "$lines")"
  said="$(grep -m1 'shortcut org.gnome.TextEditor: ' <<< "$lines" | cut -d' ' -f3-)"
  vm 'shot 80-frozen'
  check "frozen: SuperEditr answers within 3 s, and says why" "$(ok both below "$took" 3000 has "$said" "isn’t responding")" "the panel $took ms after the shortcut: $said"
  note "log: $(vm "log_since $mark | cut -d' ' -f2- | tr '\n' '|'")"
  key esc
  vm "kill -CONT $pid; wait_closed 3; sleep 1; d_focus $e; sleep 0.3; select_all $e; sleep 0.3"
  review_and_apply "after it answers again" "$e" org.gnome.TextEditor 81-thawed
  check "after it answers again: Alt+Enter applies" "$(ok has "$REVIEW_RESULT" ": applied")" "${REVIEW_RESULT#* }"
  vm 'editor_quit'
  end_case
}

# Under uwsm, Hyprland exiting ends the desktop session: SuperEditr is stopped with it, and the next
# login starts it again from its autostart entry.
restart_case() {
  begin_case hyprland-restart "Hyprland exits and the session starts again"
  local old new binds count opened
  old="$(vm 'echo $HYPRLAND_INSTANCE_SIGNATURE')"
  vm 'hyprctl dispatch "hl.dsp.exit()" > /dev/null' || true
  sleep 5
  check "SuperEditr stops with the session" "$(ok is "$(vm 'pgrep -x supereditr | wc -l' | tr -d ' ')" 0)"
  # SDDM logs in by itself only when it starts.
  vm 'sudo systemctl restart sddm' || true
  for _ in $(seq 60); do
    new="$(vm 'echo $HYPRLAND_INSTANCE_SIGNATURE' 2> /dev/null)"
    [[ -n "$new" && "$new" != "$old" ]] && vm 'hyprctl monitors > /dev/null 2>&1' && break
    sleep 2
  done
  check "a new Hyprland is running" "$(ok both isnt "$new" "" isnt "$new" "$old")"
  binds="$(vm 'for _ in $(seq 100); do [ "$(d_binds)" -ge 2 ] && break; sleep 0.2; done; sleep 3; d_binds')"
  count="$(vm 'pgrep -x supereditr | wc -l' | tr -d ' ')"
  check "the login starts SuperEditr once, with its binds once" "$(ok both is "$count" 1 is "$binds" 2)" "$count running, $binds binds"
  opened="$(vm 'd_addr "^supereditr \| SuperEditr Settings$"')"
  check "Settings doesn't open at login once Omarchy is set up" "$(ok is "$opened" null)"
  vm 'sleep 3; shot 85-after-restart'
  local e
  e="$(start_editor "" smoke-restart.txt | tail -1)"
  vm "select_all $e"
  review_and_apply "after the restart" "$e" org.gnome.TextEditor 86-after-restart
  check "after the restart: Alt+Enter applies" "$(ok has "$REVIEW_RESULT" ": applied")" "${REVIEW_RESULT#* }"
  vm 'editor_quit'
  end_case
}

# Installed here rather than kept in the baseline: it's large, and only this case needs it.
vs_code_case() {
  begin_case vs-code "VS Code, started with the flag Set up Omarchy added to its own flag file"
  local e win pid value tries file=/tmp/smoke-vscode.txt
  vm 'sudo pacman -Sy > /dev/null 2>&1; sudo pacman -S --noconfirm --needed visual-studio-code-bin > /dev/null 2>&1' || true
  e="$(vm "printf '%s' \"$SAMPLE\" > $file; d_exec 'code --new-window --disable-workspace-trust $file'
    e=\$(wait_for_window '^com.microsoft.VSCode \\| smoke-vscode' 45); d_focus \$e; echo \$e" | tail -1)"
  win="$(vm d_active)"
  pid="$(jq -r .pid <<< "$win")"
  value="$(vm "tr '\\0' ' ' < /proc/$pid/cmdline")"
  check "VS Code's launcher passes the flag" "$(ok has "$value" "--force-renderer-accessibility")" "${value:0:100}"
  # VS Code exposes the editor a moment after it draws, and only once it's clicked.
  for tries in 1 2 3 4 5 6 7 8 9 10; do
    click "$(jq -r '.at[0] + .size[0] / 2 | floor' <<< "$win")" "$(jq -r '.at[1] + .size[1] / 3 | floor' <<< "$win")"
    sleep 1
    vm "field $pid | jq -e '.chars > 0'" > /dev/null && break
  done
  note "the editor exposed its text after $tries clicks"
  key ctrl-a
  sleep 0.5
  review_and_apply "VS Code" "$e" com.microsoft.VSCode 70-vs-code
  check "VS Code: read through AT-SPI, pasting allowed" "$(ok has "$REVIEW_READ" "via AT-SPI.*pasting allowed")" "${REVIEW_READ#* }"
  check "VS Code: Alt+Enter applies" "$(ok has "$REVIEW_RESULT" ": applied")" "${REVIEW_RESULT#* }"
  key ctrl-s
  sleep 1
  value="$(vm "cat $file")"
  check "VS Code: the saved file holds the fix" "$(ok has "$value" "They're going")" "${value:0:80}"
  vm 'pkill -x code; sleep 1' || true
  end_case
}

hard_cases() {
  wants monitors && monitors_case
  wants xwayland && xwayland_case
  wants fcitx5 && fcitx_case
  wants large && large_case
  wants frozen && frozen_case
  wants hyprland-restart && restart_case
  wants vs-code && vs_code_case
}
