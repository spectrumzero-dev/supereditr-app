//! Every Lua string SuperEditr sends Hyprland, in one place. Hyprland's config and dispatcher
//! syntax changed twice in 2026 (Lua replaced its own format in 0.55), so this file and its tests
//! are what changes when it does again. Tested on 0.56.2 (Omarchy 4.0.6) and against 0.57.0.

use super::{Shortcut, WindowRef};
use crate::double_press::{ALT, COMMAND, CONTROL, SHIFT};
use crate::platform::Rect;

/// The panel's window title on Linux, which its window rule matches: every SuperEditr window
/// shares one app ID.
pub const OVERLAY_TITLE: &str = "SuperEditr suggestions";
/// SuperEditr's windows' app ID (Hyprland's class): the program name.
pub const APP_CLASS: &str = "supereditr";
/// The ID SuperEditr registers with the portals, which its `.desktop` file is named after.
pub const PORTAL_APP_ID: &str = "app.supereditr";

/// A Lua string literal.
pub fn quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            c if c.is_control() => quoted.push_str(&format!("\\{}", c as u32)),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// A regular expression that matches exactly `text`.
fn exactly(text: &str) -> String {
    let mut escaped = String::from("^");
    for c in text.chars() {
        if "\\.^$|?*+()[]{}".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped.push('$');
    escaped
}

/// The modifiers as `send_shortcut` takes them: "CTRL SHIFT".
fn mods(modifiers: u8) -> String {
    [(COMMAND, "SUPER"), (CONTROL, "CTRL"), (ALT, "ALT"), (SHIFT, "SHIFT")]
        .iter()
        .filter(|(bit, _)| modifiers & bit != 0)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join(" ")
}

/// A key combination as a bind takes it: "SUPER + ALT + E".
pub fn bind_keys(shortcut: &Shortcut) -> String {
    let mods = mods(shortcut.modifiers);
    let mut parts: Vec<&str> = mods.split(' ').filter(|m| !m.is_empty()).collect();
    parts.push(&shortcut.key);
    parts.join(" + ")
}

/// The selector for one window.
pub fn window(window: &WindowRef) -> String {
    format!("address:{}", window.handle)
}

fn overlay_selector() -> String {
    format!("title:{}", exactly(OVERLAY_TITLE))
}

pub fn focus(selector: &str) -> String {
    format!("hl.dsp.focus({{ window = {} }})", quote(selector))
}

pub fn focus_overlay() -> String {
    focus(&overlay_selector())
}

pub fn send_shortcut(shortcut: &Shortcut, selector: &str) -> String {
    format!(
        "hl.dsp.send_shortcut({{ mods = {}, key = {}, window = {} }})",
        quote(&mods(shortcut.modifiers)),
        quote(&shortcut.key),
        quote(selector)
    )
}

pub fn move_overlay(x: i32, y: i32) -> String {
    format!("hl.dsp.window.move({{ x = {x}, y = {y}, window = {} }})", quote(&overlay_selector()))
}

/// The panel's window rule: floating above everything on every workspace, without taking focus
/// when it opens, opening at `at`. Each review replaces the rule, since the panel opens somewhere
/// new each time; the one before is turned off.
pub fn overlay_rule(at: Rect) -> String {
    format!(
        "if supereditr_overlay_rule then supereditr_overlay_rule:set_enabled(false) end \
supereditr_overlay_rule = hl.window_rule({{ match = {{ class = {}, title = {} }}, float = true, pin = true, \
no_initial_focus = true, no_anim = true, no_shadow = true, no_blur = true, border_size = 0, \
move = {}, size = {} }})",
        quote(&exactly(APP_CLASS)),
        quote(&exactly(OVERLAY_TITLE)),
        quote(&format!("{} {}", at.x.round() as i32, at.y.round() as i32)),
        quote(&format!("{} {}", at.width.round() as i32, at.height.round() as i32)),
    )
}

/// What a bind does.
pub enum BindAction<'a> {
    /// Activates a GlobalShortcuts portal shortcut ("app.supereditr:review").
    Portal(&'a str),
    /// Runs a command, as `supereditr review`.
    Exec(&'a str),
}

pub fn bind(shortcut: &Shortcut, action: BindAction, description: &str) -> String {
    let dispatcher = match action {
        BindAction::Portal(id) => format!("hl.dsp.global({})", quote(&format!("{PORTAL_APP_ID}:{id}"))),
        BindAction::Exec(command) => format!("hl.dsp.exec_cmd({})", quote(command)),
    };
    format!(
        "hl.bind({}, {dispatcher}, {{ description = {} }})",
        quote(&bind_keys(shortcut)),
        quote(&format!("SuperEditr: {description}"))
    )
}

pub fn unbind(shortcut: &Shortcut) -> String {
    format!("hl.unbind({})", quote(&bind_keys(shortcut)))
}

/// Turns off the panel's rule, when SuperEditr quits.
pub fn remove_overlay_rule() -> String {
    "if supereditr_overlay_rule then supereditr_overlay_rule:set_enabled(false) supereditr_overlay_rule = nil end"
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(modifiers: u8, key: &str) -> Shortcut {
        Shortcut { modifiers, key: key.into() }
    }

    fn target() -> WindowRef {
        WindowRef {
            handle: "0x562229d30760".into(),
            pid: 11592,
            app_id: "org.gnome.TextEditor".into(),
            title: "Notes".into(),
            rect: None,
            xwayland: false,
            tags: Vec::new(),
        }
    }

    // Hyprland 0.56.2 answers "ok" to each of these, and 0.57.0's source has the same dispatcher
    // and rule names.

    #[test]
    fn a_paste_is_ctrl_v_sent_to_the_window_by_address() {
        assert_eq!(
            send_shortcut(&key(CONTROL, "V"), &window(&target())),
            r#"hl.dsp.send_shortcut({ mods = "CTRL", key = "V", window = "address:0x562229d30760" })"#
        );
        assert_eq!(
            send_shortcut(&key(CONTROL, "Insert"), "pid:4242"),
            r#"hl.dsp.send_shortcut({ mods = "CTRL", key = "Insert", window = "pid:4242" })"#
        );
    }

    #[test]
    fn focus_and_move_select_the_window() {
        assert_eq!(focus(&window(&target())), r#"hl.dsp.focus({ window = "address:0x562229d30760" })"#);
        assert_eq!(focus_overlay(), r#"hl.dsp.focus({ window = "title:^SuperEditr suggestions$" })"#);
        assert_eq!(
            move_overlay(300, 150),
            r#"hl.dsp.window.move({ x = 300, y = 150, window = "title:^SuperEditr suggestions$" })"#
        );
    }

    #[test]
    fn the_review_keys_bind_to_the_portal_shortcut() {
        let review = key(COMMAND | ALT, "E");
        assert_eq!(bind_keys(&review), "SUPER + ALT + E");
        assert_eq!(bind_keys(&key(COMMAND | CONTROL | ALT, "E")), "SUPER + CTRL + ALT + E");
        assert_eq!(
            bind(&review, BindAction::Portal("review"), "review the selection"),
            r#"hl.bind("SUPER + ALT + E", hl.dsp.global("app.supereditr:review"), { description = "SuperEditr: review the selection" })"#
        );
        assert_eq!(
            bind(&review, BindAction::Exec("supereditr review"), "review the selection"),
            r#"hl.bind("SUPER + ALT + E", hl.dsp.exec_cmd("supereditr review"), { description = "SuperEditr: review the selection" })"#
        );
        assert_eq!(unbind(&review), r#"hl.unbind("SUPER + ALT + E")"#);
    }

    #[test]
    fn the_panel_rule_opens_it_floating_and_unfocused_where_placed() {
        let rule = overlay_rule(Rect { x: 300.4, y: 150.0, width: 548.0, height: 210.0 });
        assert!(rule.starts_with("if supereditr_overlay_rule then supereditr_overlay_rule:set_enabled(false) end "));
        assert!(rule.contains(r#"match = { class = "^supereditr$", title = "^SuperEditr suggestions$" }"#), "{rule}");
        for property in ["float = true", "pin = true", "no_initial_focus = true", "no_anim = true", "border_size = 0"] {
            assert!(rule.contains(property), "{property} in {rule}");
        }
        assert!(rule.contains(r#"move = "300 150", size = "548 210""#), "{rule}");
    }

    #[test]
    fn strings_are_quoted_and_titles_matched_literally() {
        assert_eq!(quote(r#"Rhyan's "notes" \ draft"#), r#""Rhyan's \"notes\" \\ draft""#);
        assert_eq!(quote("a\nb"), r#""a\nb""#);
        assert_eq!(exactly("SuperEditr (1).txt"), r"^SuperEditr \(1\)\.txt$");
    }
}
