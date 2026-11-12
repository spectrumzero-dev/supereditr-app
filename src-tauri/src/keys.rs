//! Shortcuts as Windows keys.
//!
//! A shortcut is saved as an accelerator string, "Control+Shift+F2": modifiers, then one key.
//! Named keys (F2, Space, letters and digits) are the same key on every layout. A key that types a
//! different character on different layouts is saved as the virtual key Windows reported when the
//! shortcut was recorded, `Vk` and two hex digits ("Control+Shift+VkBA" is the key that types ü on
//! a German layout and ; on a US one), and registered as that virtual key. Windows matches a hotkey
//! by virtual key in the layout of the app in front, so the shortcut fires on the key the user
//! pressed. Key names written by the macOS recorder ("Semicolon", "BracketLeft") are US key
//! positions and read as the US layout's virtual keys.

use crate::double_press::{ALT, COMMAND, CONTROL, SHIFT};

/// One chord, as Windows registers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    /// `double_press` modifier bits; COMMAND is the Windows key.
    pub modifiers: u8,
    /// The Windows virtual key.
    pub vk: u16,
}

const NAMED: &[(&str, u16)] = &[
    ("Space", 0x20),
    ("Enter", 0x0D),
    ("Return", 0x0D),
    ("Tab", 0x09),
    ("Escape", 0x1B),
    ("Esc", 0x1B),
    ("Backspace", 0x08),
    ("Delete", 0x2E),
    ("Insert", 0x2D),
    ("Home", 0x24),
    ("End", 0x23),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("ArrowLeft", 0x25),
    ("ArrowUp", 0x26),
    ("ArrowRight", 0x27),
    ("ArrowDown", 0x28),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("PrintScreen", 0x2C),
    ("Pause", 0x13),
    // US key positions, as the macOS recorder names them.
    ("Semicolon", 0xBA),
    ("Equal", 0xBB),
    ("Comma", 0xBC),
    ("Minus", 0xBD),
    ("Period", 0xBE),
    ("Slash", 0xBF),
    ("Backquote", 0xC0),
    ("BracketLeft", 0xDB),
    ("Backslash", 0xDC),
    ("BracketRight", 0xDD),
    ("Quote", 0xDE),
    ("IntlBackslash", 0xE2),
];

/// The virtual key for a key's name in an accelerator.
pub fn vk(name: &str) -> Option<u16> {
    let upper = name.to_ascii_uppercase();
    if upper.len() == 1 && upper.as_bytes()[0].is_ascii_alphanumeric() {
        return Some(u16::from(upper.as_bytes()[0]));
    }
    if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<u16>().ok())
        && (1..=24).contains(&n)
    {
        return Some(0x6F + n);
    }
    if let Some(hex) = name.strip_prefix("Vk").or_else(|| name.strip_prefix("VK"))
        && hex.len() == 2
    {
        return u16::from_str_radix(hex, 16).ok().filter(|&vk| vk > 0 && vk < 0xFF);
    }
    NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|&(_, vk)| vk)
}

/// The accelerator name for a virtual key: its own name when it has one, otherwise `VkXX`.
pub fn name(vk: u16) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from(vk as u8).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => match NAMED.iter().find(|&&(n, v)| v == vk && !(0xBA..=0xE2).contains(&v) && n.len() > 2) {
            Some((n, _)) => (*n).to_string(),
            None => format!("Vk{vk:02X}"),
        },
    }
}

/// A one-chord accelerator as Windows keys. None for a double press, or a key Windows can't name.
pub fn chord(accelerator: &str) -> Option<Chord> {
    let mut parts = accelerator.split_whitespace();
    let one = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let mut modifiers = 0;
    let mut key = None;
    for part in one.split('+') {
        match part.to_ascii_lowercase().as_str() {
            "control" | "ctrl" | "cmdorctrl" => modifiers |= CONTROL,
            "alt" | "option" => modifiers |= ALT,
            "shift" => modifiers |= SHIFT,
            "command" | "cmd" | "super" | "win" | "meta" => modifiers |= COMMAND,
            _ if key.is_some() => return None,
            _ => key = Some(vk(part)?),
        }
    }
    Some(Chord { modifiers, vk: key? })
}

/// Whether a virtual key is a function key.
pub fn is_function_key(vk: u16) -> bool {
    (0x70..=0x87).contains(&vk)
}

/// Whether a virtual key types a character: letters, digits and the punctuation keys.
pub fn types_character(vk: u16) -> bool {
    matches!(vk, 0x30..=0x39 | 0x41..=0x5A | 0xBA..=0xC0 | 0xDB..=0xDF | 0xE1..=0xE4)
}

/// Shortcuts Windows or common apps use, which registration accepts all the same.
/// `blocks` is false for one SuperEditr knowingly keeps: Ctrl+Shift+F2 is its default.
const IN_USE: &[(u8, u16, &str, bool)] = &[
    (ALT, 0x20, "Windows uses this for a window’s menu", true),
    (CONTROL, 0x1B, "Windows uses this to open Start", true),
    (CONTROL | SHIFT, 0x1B, "Windows uses this to open Task Manager", true),
    (0, 0x2C, "Windows uses this for screenshots", true),
    (ALT, 0x2C, "Windows uses this for screenshots", true),
    (CONTROL, 0x45, "Word and web browsers use this (centre text, the address bar)", true),
    (CONTROL | SHIFT, 0x20, "Word uses this to type a non-breaking space", true),
    (CONTROL | SHIFT, 0x71, "Excel uses this for a threaded comment", false),
    (CONTROL | SHIFT, 0x70, "Excel uses this to show or hide the ribbon", true),
    (CONTROL | SHIFT, 0xBA, "Excel uses this to enter the time, and VS Code to focus breadcrumbs", true),
    (CONTROL | SHIFT, 0x72, "Word uses this to change capitalisation", true),
    (CONTROL | SHIFT, 0x74, "Word uses this for bookmarks", true),
    (CONTROL | SHIFT, 0x75, "Word uses this to move between windows", true),
    (CONTROL | SHIFT, 0x76, "Word uses this to unlink fields", true),
    (CONTROL | SHIFT, 0x77, "Word uses this to extend a selection", true),
    (CONTROL | SHIFT, 0x78, "Word uses this to unlink fields", true),
    (CONTROL | SHIFT, 0x7A, "Word uses this to unlock fields", true),
    (CONTROL | SHIFT, 0x7B, "Word uses this to print", true),
    (CONTROL | SHIFT, 0xBE, "Word, Edge (Copilot) and VS Code use this", true),
    (CONTROL | SHIFT, 0xDB, "VS Code uses this to fold code", true),
    (CONTROL | SHIFT, 0xDD, "VS Code uses this to unfold code", true),
];

/// Why `accelerator` can't be a Windows shortcut, in words for the user, or None if it's free.
/// `others` are SuperEditr's other shortcuts and what each does.
pub fn windows_conflict(accelerator: &str, others: &[(String, String)]) -> Option<String> {
    if accelerator.split_whitespace().count() > 1 {
        return Some("Double presses work only on macOS for now.".into());
    }
    let Some(this) = chord(accelerator) else {
        return Some("SuperEditr can’t use this key combination.".into());
    };
    if this.modifiers & COMMAND != 0 {
        return Some("Windows keeps shortcuts with the Windows key for itself. Use Ctrl or Alt.".into());
    }
    if this.modifiers & (CONTROL | ALT) == 0 && !is_function_key(this.vk) {
        return Some("Add Ctrl or Alt, or it would fire whenever you type this key.".into());
    }
    if this.modifiers & CONTROL != 0 && this.modifiers & ALT != 0 && types_character(this.vk) {
        return Some(
            "Ctrl+Alt with a letter or symbol is AltGr on many keyboards, which types characters such as € or @."
                .into(),
        );
    }
    if let Some((_, _, what, _)) =
        IN_USE.iter().find(|&&(m, vk, _, blocks)| blocks && m == this.modifiers && vk == this.vk)
    {
        return Some(format!("{what}. Choose another."));
    }
    for (other, what) in others {
        if chord(other).is_some_and(|o| o == this) {
            return Some(format!("SuperEditr already uses this for {what}."));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_recorded_keys_become_virtual_keys() {
        assert_eq!(chord("Control+Shift+F2"), Some(Chord { modifiers: CONTROL | SHIFT, vk: 0x71 }));
        assert_eq!(chord("Ctrl+Alt+Space"), Some(Chord { modifiers: CONTROL | ALT, vk: 0x20 }));
        assert_eq!(chord("Control+Shift+VkBA"), Some(Chord { modifiers: CONTROL | SHIFT, vk: 0xBA }));
        assert_eq!(chord("Control+Shift+Semicolon"), chord("Control+Shift+VkBA"), "a US position");
        assert_eq!(chord("Alt+k"), Some(Chord { modifiers: ALT, vk: 0x4B }));
        assert_eq!(chord("Control+F24").map(|c| c.vk), Some(0x87));
        assert_eq!(chord("Command+E Command+E"), None, "a double press isn't a chord");
        assert_eq!(chord("Control+Shift"), None, "no key");
        assert_eq!(chord("Control+F25"), None);
        assert_eq!(chord("Control+VkZZ"), None);
    }

    #[test]
    fn a_virtual_key_is_named_as_the_recorder_writes_it() {
        assert_eq!(name(0x41), "A");
        assert_eq!(name(0x37), "7");
        assert_eq!(name(0x71), "F2");
        assert_eq!(name(0x20), "Space");
        assert_eq!(name(0xBA), "VkBA", "a punctuation key keeps its virtual key, not a US name");
        assert_eq!(name(0xDE), "VkDE");
        for vk in [0x20, 0x41, 0x71, 0xBA, 0xE2, 0x2E] {
            assert_eq!(super::vk(&name(vk)), Some(vk), "{vk:#x} round-trips");
        }
    }

    fn check(accelerator: &str) -> Option<String> {
        windows_conflict(accelerator, &[("Control+Alt+F9".into(), "Quick fix".into())])
    }

    #[test]
    fn the_default_and_ordinary_chords_are_free() {
        assert_eq!(check("Control+Shift+F2"), None, "the default, though Excel has it too");
        assert_eq!(check("Control+Alt+Space"), None, "the layout-safe alternative");
        assert_eq!(check("F6"), None, "a lone function key");
        assert_eq!(check("Control+Shift+K"), None);
        assert_eq!(check("Alt+Shift+VkBA"), None);
    }

    #[test]
    fn windows_and_app_shortcuts_are_refused() {
        assert!(check("Command+E").unwrap().contains("Windows key"));
        assert!(check("Shift+E").unwrap().contains("Add Ctrl or Alt"));
        assert!(check("Control+Alt+E").unwrap().contains("AltGr"));
        assert!(check("Control+Alt+VkBA").unwrap().contains("AltGr"));
        assert!(check("Alt+Space").unwrap().contains("window’s menu"));
        assert!(check("Control+E").unwrap().contains("Word"));
        assert!(check("Control+Shift+Space").unwrap().contains("non-breaking"));
        assert!(check("Control+Shift+Period").unwrap().contains("Copilot"));
        assert!(check("Control+Shift+VkDB").unwrap().contains("VS Code"));
        assert!(check("Control+Shift+F12").unwrap().contains("Word"));
        assert!(check("Command+E Command+E").unwrap().contains("macOS"));
    }

    #[test]
    fn supereditrs_own_shortcuts_are_refused() {
        assert!(check("Control+Alt+F9").unwrap().contains("Quick fix"));
        assert!(check("Ctrl+Alt+F9").unwrap().contains("Quick fix"), "however it's spelled");
    }
}
