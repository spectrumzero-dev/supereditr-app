# shellcheck shell=bash
# The Linux smoke tests' desktop layer for Hyprland (0.55 and later, Lua dispatchers), sourced on the
# test machine by vm.sh. Another desktop gets its own file defining the same d_* functions.
#
# Keys sent here go straight to one window through Hyprland's send_shortcut, for keys the test
# itself presses in an app (select all, undo). Keys meant as the user's own come from the virtual
# machine's keyboard instead (see scripts/ui-smoke-linux.sh). Never wtype: after a wtype keystroke,
# send_shortcut fails until a real key is pressed.

d_name() { echo "Hyprland $(hyprctl version -j | jq -r .tag)"; }

# Every window, as Hyprland's JSON.
d_windows() { hyprctl clients -j; }

# The address of the first window whose "class | title" matches a regular expression, or "null".
d_addr() { hyprctl clients -j | jq -r --arg re "$1" '[.[] | select((.class + " | " + .title) | test($re))][0].address'; }

d_pid() { hyprctl clients -j | jq -r --arg a "$1" '.[] | select(.address == $a) | .pid'; }

d_focus() { hyprctl dispatch "hl.dsp.focus({ window = \"address:$1\" })" > /dev/null; }

# d_keys ADDRESS MODS KEY, such as d_keys 0x55d0 CTRL A.
d_keys() { hyprctl dispatch "hl.dsp.send_shortcut({ mods = \"$2\", key = \"$3\", window = \"address:$1\" })" > /dev/null; }

# The window in front: {address, class, title, pid, at, size, monitor, xwayland}.
d_active() { hyprctl activewindow -j | jq -c '{address, class, title, pid, at, size, monitor, xwayland}'; }

# SuperEditr's panel: {shown, focused, at, size, monitor}; shown is false when it's hidden or gone.
d_panel() {
  local active
  active="$(hyprctl activewindow -j | jq -r .title)"
  hyprctl clients -j | jq -c --arg active "$active" '
    [.[] | select(.class == "supereditr" and .title == "SuperEditr suggestions")][0] as $p
    | if $p == null then {shown: false, focused: false}
      else {shown: ($p.mapped and ($p.hidden | not)), focused: ($active == "SuperEditr suggestions"),
            at: $p.at, size: $p.size, monitor: $p.monitor} end'
}

# Monitors: [{id, name, x, y, width, height, scale}] with width and height in logical pixels.
d_monitors() { hyprctl monitors -j | jq -c '[.[] | {id, name, x, y, width: ((.width / .scale) | floor), height: ((.height / .scale) | floor), scale}]'; }

d_cursor() { hyprctl cursorpos -j | jq -c '[.x, .y]'; }

d_cursor_move() { hyprctl dispatch "hl.dsp.cursor.move({ x = $1, y = $2 })" > /dev/null; }

d_reload() { hyprctl reload > /dev/null; }

# How many of Hyprland's binds are SuperEditr's.
d_binds() { hyprctl binds -j | jq '[.[] | select(.description | startswith("SuperEditr:"))] | length'; }

# Starts a program as a desktop app (uwsm puts it in the session's app slice).
d_exec() { hyprctl dispatch "hl.dsp.exec_cmd(\"uwsm app -- $1\")" > /dev/null; }

# d_to_workspace ADDRESS WORKSPACE moves a window to a workspace without following it.
d_to_workspace() { hyprctl dispatch "hl.dsp.window.move({ workspace = \"$2\", window = \"address:$1\", follow = false })" > /dev/null; }

d_workspace() { hyprctl dispatch "hl.dsp.focus({ workspace = \"$1\" })" > /dev/null; }

# A second output with no screen behind it, which grim can still capture.
d_output_add() { hyprctl output create headless "$1" > /dev/null; }
d_output_remove() { hyprctl output remove "$1" > /dev/null; }

# d_monitor NAME MODE POSITION SCALE, such as d_monitor HEADLESS-2 1600x900 1280x0 1.5.
d_monitor() { hyprctl eval "hl.monitor({ output = \"$1\", mode = \"$2\", position = \"$3\", scale = $4 })" > /dev/null; }
