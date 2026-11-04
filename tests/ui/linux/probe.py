#!/usr/bin/env python3
"""What the accessibility bus (AT-SPI) shows the smoke tests, as one JSON line.

  probe.py focused PID     the focused text field in the app with that pid: role, editable,
                           password, its whole text, the selection and where the selection is, as
                           the app reports it: [x, y, width, height], or null without a position
  probe.py editable TITLE  the frame of the first editable text field in the SuperEditr window with
                           that title, inside the window: [x, y, width, height]
  probe.py panel           the text of SuperEditr's suggestions panel, "" when it has none
  probe.py press TITLE BUTTON
                           presses a button in the SuperEditr window with that title

Runs on the test machine, in the desktop session. Every call has a time limit, so a frozen app
makes the probe fail instead of hang.
"""
import json
import sys

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402

MAX_NODES = 20000
PANEL_TITLE = "SuperEditr suggestions"


def apps():
    # Milliseconds for each call, and for the first call to an app that is starting up.
    Atspi.set_timeout(1000, 2000)
    desktop = Atspi.get_desktop(0)
    for i in range(desktop.get_child_count()):
        app = desktop.get_child_at_index(i)
        if app is None:
            continue
        try:
            yield app, app.get_process_id(), app.get_name() or ""
        except Exception:
            continue


def walk(root):
    stack, seen = [root], 0
    while stack and seen < MAX_NODES:
        node = stack.pop()
        seen += 1
        yield node
        try:
            if node.get_state_set().contains(Atspi.StateType.DEFUNCT):
                continue
            count = node.get_child_count()
        except Exception:
            continue
        for i in range(count - 1, -1, -1):
            try:
                child = node.get_child_at_index(i)
            except Exception:
                child = None
            if child is not None:
                stack.append(child)


def range_rect(node, start, end):
    """The same answer SuperEditr's own read accepts as a position (platform/linux/atspi.rs)."""
    for coordinates in (Atspi.CoordType.SCREEN, Atspi.CoordType.WINDOW):
        r = Atspi.Text.get_range_extents(node, start, max(end, start + 1), coordinates)
        if r.width > 0 and r.height > 0 and r.x >= 0 and r.y >= 0 and (r.x, r.y) != (0, 0):
            return [r.x, r.y, r.width, r.height]
    return None


def focused(pid):
    app = next((app for app, p, _ in apps() if p == pid), None)
    if app is None:
        return {"error": f"no app with pid {pid} on the accessibility bus"}
    for node in walk(app):
        try:
            if node is app or not node.get_state_set().contains(Atspi.StateType.FOCUSED):
                continue
            states = node.get_state_set()
            out = {
                "role": node.get_role_name(),
                "editable": states.contains(Atspi.StateType.EDITABLE),
                "password": node.get_role() == Atspi.Role.PASSWORD_TEXT,
            }
            if node.get_text_iface() is not None:
                count = Atspi.Text.get_character_count(node)
                out["chars"] = count
                out["text"] = Atspi.Text.get_text(node, 0, count)
                if Atspi.Text.get_n_selections(node) > 0:
                    r = Atspi.Text.get_selection(node, 0)
                    out["selection"] = [r.start_offset, r.end_offset]
                    out["selection_rect"] = range_rect(node, r.start_offset, r.end_offset)
            return out
        except Exception as error:
            return {"error": str(error)}
    return {"error": "nothing focused"}


def panel():
    for app, _, name in apps():
        if "supereditr" not in name.lower():
            continue
        for w in range(app.get_child_count()):
            window = app.get_child_at_index(w)
            if window is None or window.get_name() != PANEL_TITLE:
                continue
            parts = []
            for node in walk(window):
                try:
                    if node.get_child_count() == 0 and node.get_text_iface() is not None:
                        text = Atspi.Text.get_text(node, 0, -1).strip()
                        if text:
                            parts.append(text)
                except Exception:
                    pass
            return {"text": " | ".join(parts)}
    return {"text": ""}


def press(window_title, button):
    """Presses a button in one of SuperEditr's windows through its accessible action."""
    for app, _, name in apps():
        if "supereditr" not in name.lower():
            continue
        for w in range(app.get_child_count()):
            window = app.get_child_at_index(w)
            if window is None or window.get_name() != window_title:
                continue
            for node in walk(window):
                try:
                    if node.get_role() == Atspi.Role.BUTTON and (node.get_name() or "").strip() == button:
                        return {"pressed": Atspi.Action.do_action(node, 0)}
                except Exception as error:
                    return {"error": str(error)}
            return {"error": f"no button {button!r} in {window_title!r}"}
    return {"error": f"no window {window_title!r}"}


def editable(window_title):
    for app, _, name in apps():
        if "supereditr" not in name.lower():
            continue
        for w in range(app.get_child_count()):
            window = app.get_child_at_index(w)
            if window is None or window.get_name() != window_title:
                continue
            for node in walk(window):
                try:
                    if node.get_state_set().contains(Atspi.StateType.EDITABLE):
                        r = Atspi.Component.get_extents(node, Atspi.CoordType.WINDOW)
                        return {"rect": [r.x, r.y, r.width, r.height]}
                except Exception as error:
                    return {"error": str(error)}
            return {"error": f"no editable field in {window_title!r}"}
    return {"error": f"no window {window_title!r}"}


def main():
    what = sys.argv[1] if len(sys.argv) > 1 else ""
    if what == "focused" and len(sys.argv) > 2:
        result = focused(int(sys.argv[2]))
    elif what == "editable" and len(sys.argv) > 2:
        result = editable(sys.argv[2])
    elif what == "panel":
        result = panel()
    elif what == "press" and len(sys.argv) > 3:
        result = press(sys.argv[2], sys.argv[3])
    else:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
