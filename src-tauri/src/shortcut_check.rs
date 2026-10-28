//! Refuses a new shortcut that something else already answers to: macOS itself (Spotlight,
//! screenshots, input sources, and any system shortcut turned on in System Settings), the ⌘
//! commands every app has, SuperEditr's own other shortcuts, or another app's global hotkey.
//!
//! A shortcut is one press ("Control+Alt+E", or a lone function key such as "F6") or a double press
//! ("Command+E Command+E"). A double press may reuse an app command such as ⌘E, because a single
//! press still reaches the app (see `double_press`), but not a system shortcut, which macOS acts on
//! before any app sees the key.

use crate::double_press::{ALT, COMMAND, CONTROL, SHIFT};

/// One key with its modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Combo {
    pub modifiers: u8,
    /// macOS virtual key code.
    pub keycode: u16,
}

/// A recorded shortcut: one press or the same press twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub combo: Combo,
    pub double: bool,
}

/// Key names as `acceleratorFromEvent` writes them (ui/src/lib/accelerator.ts), with macOS key codes.
const KEYS: &[(&str, u16)] = &[
    ("A", 0),
    ("S", 1),
    ("D", 2),
    ("F", 3),
    ("H", 4),
    ("G", 5),
    ("Z", 6),
    ("X", 7),
    ("C", 8),
    ("V", 9),
    ("B", 11),
    ("Q", 12),
    ("W", 13),
    ("E", 14),
    ("R", 15),
    ("Y", 16),
    ("T", 17),
    ("1", 18),
    ("2", 19),
    ("3", 20),
    ("4", 21),
    ("6", 22),
    ("5", 23),
    ("Equal", 24),
    ("9", 25),
    ("7", 26),
    ("Minus", 27),
    ("8", 28),
    ("0", 29),
    ("BracketRight", 30),
    ("O", 31),
    ("U", 32),
    ("BracketLeft", 33),
    ("I", 34),
    ("P", 35),
    ("Enter", 36),
    ("L", 37),
    ("J", 38),
    ("Quote", 39),
    ("K", 40),
    ("Semicolon", 41),
    ("Backslash", 42),
    ("Comma", 43),
    ("Slash", 44),
    ("N", 45),
    ("M", 46),
    ("Period", 47),
    ("Tab", 48),
    ("Space", 49),
    ("Backquote", 50),
    ("Escape", 53),
    ("F1", 122),
    ("F2", 120),
    ("F3", 99),
    ("F4", 118),
    ("F5", 96),
    ("F6", 97),
    ("F7", 98),
    ("F8", 100),
    ("F9", 101),
    ("F10", 109),
    ("F11", 103),
    ("F12", 111),
    ("F13", 105),
    ("F14", 107),
    ("F15", 113),
    ("F16", 106),
    ("F17", 64),
    ("F18", 79),
    ("F19", 80),
    ("F20", 90),
];

fn keycode(name: &str) -> Option<u16> {
    KEYS.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|&(_, code)| code)
}

fn is_function_key(keycode: u16) -> bool {
    KEYS.iter().any(|&(name, code)| code == keycode && name.len() > 1 && name.starts_with('F'))
}

fn parse_combo(text: &str) -> Option<Combo> {
    let mut modifiers = 0;
    let mut key = None;
    for part in text.split('+') {
        match part.to_ascii_lowercase().as_str() {
            "command" | "cmd" | "super" | "cmdorctrl" => modifiers |= COMMAND,
            "control" | "ctrl" => modifiers |= CONTROL,
            "alt" | "option" => modifiers |= ALT,
            "shift" => modifiers |= SHIFT,
            _ => key = Some(keycode(part)?),
        }
    }
    Some(Combo { modifiers, keycode: key? })
}

pub fn parse(accelerator: &str) -> Option<Parsed> {
    let parts: Vec<&str> = accelerator.split_whitespace().collect();
    match parts.as_slice() {
        [one] => Some(Parsed { combo: parse_combo(one)?, double: false }),
        [first, second] if first.eq_ignore_ascii_case(second) => {
            Some(Parsed { combo: parse_combo(first)?, double: true })
        }
        _ => None,
    }
}

/// macOS's own shortcuts that are on by default, by their id in `com.apple.symbolichotkeys`
/// (`None` for ones handled elsewhere, which can't be turned off there).
const SYSTEM: &[(Option<u32>, u8, u16, &str)] = &[
    (Some(64), COMMAND, 49, "Spotlight"),
    (Some(65), COMMAND | ALT, 49, "Finder search"),
    (Some(60), CONTROL, 49, "switching input sources"),
    (Some(61), CONTROL | ALT, 49, "switching input sources"),
    (Some(28), COMMAND | SHIFT, 20, "screenshots"),
    (Some(29), COMMAND | CONTROL | SHIFT, 20, "screenshots"),
    (Some(30), COMMAND | SHIFT, 21, "screenshots"),
    (Some(31), COMMAND | CONTROL | SHIFT, 21, "screenshots"),
    (Some(184), COMMAND | SHIFT, 23, "screenshots and recording"),
    (Some(27), COMMAND, 50, "switching windows"),
    (Some(36), 0, 103, "Show Desktop"),
    (Some(52), COMMAND | ALT, 2, "showing and hiding the Dock"),
    (Some(98), COMMAND | SHIFT, 44, "the Help menu"),
    (Some(162), COMMAND | ALT, 96, "accessibility controls"),
    (Some(12), CONTROL, 122, "keyboard navigation"),
    (Some(7), CONTROL, 120, "keyboard navigation"),
    (Some(8), CONTROL, 99, "keyboard navigation"),
    (Some(9), CONTROL, 118, "keyboard navigation"),
    (Some(10), CONTROL, 96, "keyboard navigation"),
    (Some(11), CONTROL, 97, "keyboard navigation"),
    (Some(13), CONTROL, 98, "keyboard navigation"),
    (Some(57), CONTROL, 100, "keyboard navigation"),
    (None, COMMAND | CONTROL, 12, "locking the screen"),
    (None, COMMAND | SHIFT, 12, "logging out"),
    (None, COMMAND | CONTROL, 49, "the emoji picker"),
    (None, COMMAND | CONTROL, 3, "full screen"),
    (None, COMMAND, 48, "switching apps"),
];

/// A system shortcut as the user has it set in System Settings → Keyboard → Keyboard Shortcuts.
pub struct UserHotkey {
    pub id: u32,
    pub enabled: bool,
    pub combo: Option<Combo>,
}

/// Converts `com.apple.symbolichotkeys` modifier flags. `None` for a shortcut that uses fn, which
/// SuperEditr's shortcuts never do.
fn modifiers_from_flags(flags: u64) -> Option<u8> {
    if flags & (1 << 23) != 0 {
        return None;
    }
    let mut m = 0;
    if flags & (1 << 17) != 0 {
        m |= SHIFT;
    }
    if flags & (1 << 18) != 0 {
        m |= CONTROL;
    }
    if flags & (1 << 19) != 0 {
        m |= ALT;
    }
    if flags & (1 << 20) != 0 {
        m |= COMMAND;
    }
    Some(m)
}

/// Reads the system shortcuts the user has changed or turned on (an XML property list from
/// `defaults export com.apple.symbolichotkeys -`).
pub fn parse_symbolic_hotkeys(xml: &[u8]) -> Vec<UserHotkey> {
    let Ok(root) = plist::Value::from_reader_xml(xml) else { return Vec::new() };
    let Some(keys) = root.as_dictionary().and_then(|d| d.get("AppleSymbolicHotKeys")).and_then(|v| v.as_dictionary())
    else {
        return Vec::new();
    };
    keys.iter()
        .filter_map(|(id, entry)| {
            let id = id.parse().ok()?;
            let entry = entry.as_dictionary()?;
            let enabled = match entry.get("enabled")? {
                plist::Value::Boolean(b) => *b,
                v => v.as_signed_integer().is_some_and(|n| n != 0),
            };
            let params = entry.get("value").and_then(|v| v.as_dictionary()).and_then(|v| v.get("parameters"));
            let combo = params.and_then(|p| p.as_array()).and_then(|p| {
                let keycode = p.get(1)?.as_signed_integer()?;
                let flags = p.get(2)?.as_signed_integer()?;
                (keycode != 65535).then_some(())?;
                Some(Combo { modifiers: modifiers_from_flags(flags as u64)?, keycode: keycode as u16 })
            });
            Some(UserHotkey { id, enabled, combo })
        })
        .collect()
}

#[cfg(target_os = "macos")]
pub fn user_hotkeys() -> Vec<UserHotkey> {
    std::process::Command::new("/usr/bin/defaults")
        .args(["export", "com.apple.symbolichotkeys", "-"])
        .output()
        .map(|out| parse_symbolic_hotkeys(&out.stdout))
        .unwrap_or_default()
}

#[cfg(not(target_os = "macos"))]
pub fn user_hotkeys() -> Vec<UserHotkey> {
    Vec::new()
}

/// What macOS uses this key for, if anything: the defaults above as the user left them, plus any
/// other system shortcut they turned on.
pub fn system_use(combo: Combo, user: &[UserHotkey]) -> Option<&'static str> {
    for &(id, modifiers, keycode, label) in SYSTEM {
        let current = match id.and_then(|id| user.iter().find(|u| u.id == id)) {
            Some(u) if !u.enabled => None,
            Some(u) => u.combo.or(Some(Combo { modifiers, keycode })),
            None => Some(Combo { modifiers, keycode }),
        };
        if current == Some(combo) {
            return Some(label);
        }
    }
    user.iter().any(|u| u.enabled && u.combo == Some(combo)).then_some("a keyboard shortcut")
}

/// Why `accelerator` can't be used, in words for the user, or `None` if it's free. `others` are
/// SuperEditr's other shortcuts and what each does.
pub fn conflict(accelerator: &str, others: &[(String, String)], user: &[UserHotkey]) -> Option<String> {
    let Some(shortcut) = parse(accelerator) else {
        return Some("SuperEditr can’t use this key combination.".into());
    };
    let combo = shortcut.combo;
    if combo.modifiers & !SHIFT == 0 && !is_function_key(combo.keycode) {
        return Some("Add ⌘, ⌃, or ⌥, or it would fire whenever you type this key.".into());
    }
    if let Some(what) = system_use(combo, user) {
        return Some(format!(
            "macOS uses this for {what}. Choose another, or turn it off in System Settings → Keyboard → Keyboard \
             Shortcuts."
        ));
    }
    if !shortcut.double && combo.modifiers & (CONTROL | ALT) == 0 && combo.modifiers & COMMAND != 0 {
        let hint = if keycode_is_letter_or_digit(combo.keycode) { ", or press it twice, like ⌘E E" } else { "" };
        return Some(format!(
            "Apps use ⌘ shortcuts like this for their own commands, and SuperEditr would take it from all of them. \
             Add ⌃ or ⌥{hint}."
        ));
    }
    for (other, what) in others {
        let Some(o) = parse(other) else { continue };
        // Also a single press of a chord that's another's double press: it would fire first.
        if o.combo == combo {
            return Some(format!("SuperEditr already uses this for {what}."));
        }
    }
    None
}

fn keycode_is_letter_or_digit(keycode: u16) -> bool {
    KEYS.iter().any(|&(name, code)| code == keycode && name.len() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(accelerator: &str) -> Option<String> {
        conflict(accelerator, &[("Control+Alt+Shift+E".into(), "Quick fix".into())], &[])
    }

    #[test]
    fn allows_ordinary_shortcuts() {
        assert_eq!(check("Control+Alt+E"), None);
        assert_eq!(check("Command+E Command+E"), None, "a double press may reuse an app command");
        assert_eq!(check("F6"), None, "a lone function key");
        assert_eq!(check("Command+Alt+K"), None);
    }

    #[test]
    fn refuses_plain_keys_and_app_commands() {
        assert!(check("E").unwrap().contains("Add ⌘"));
        assert!(check("Shift+E").unwrap().contains("Add ⌘"));
        assert!(check("Command+C").unwrap().contains("press it twice"));
        assert!(check("Command+Shift+Period").unwrap().contains("Apps use"));
    }

    #[test]
    fn refuses_system_shortcuts_even_pressed_twice() {
        assert!(check("Command+Space").unwrap().contains("Spotlight"));
        assert!(check("Control+Space Control+Space").unwrap().contains("input sources"));
        assert!(check("Command+Shift+4").unwrap().contains("screenshots"));
    }

    #[test]
    fn refuses_supereditrs_own_shortcuts() {
        assert!(check("Control+Alt+Shift+E").unwrap().contains("Quick fix"));
        let others = [("Command+E Command+E".to_string(), "Review selection".to_string())];
        assert!(conflict("Command+Alt+E Command+Alt+E", &others, &[]).is_none());
        let others = [("Control+Alt+E".to_string(), "Quick fix".to_string())];
        assert!(conflict("Control+Alt+E Control+Alt+E", &others, &[]).unwrap().contains("Quick fix"));
    }

    #[test]
    fn follows_the_users_system_settings() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>AppleSymbolicHotKeys</key><dict>
  <key>64</key><dict><key>enabled</key><false/></dict>
  <key>118</key><dict><key>enabled</key><true/><key>value</key><dict><key>parameters</key>
    <array><integer>65535</integer><integer>18</integer><integer>262144</integer></array>
    <key>type</key><string>standard</string></dict></dict>
</dict></dict></plist>"#;
        let user = parse_symbolic_hotkeys(xml);
        assert_eq!(user.len(), 2);
        let others: &[(String, String)] = &[];
        assert!(conflict("Command+Space Command+Space", others, &user).is_none(), "Spotlight's shortcut is off");
        assert!(conflict("Control+1 Control+1", others, &user).unwrap().contains("macOS"), "⌃1 turned on");
    }
}
