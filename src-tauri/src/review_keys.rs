//! Keys for a review the user opened (⌘E E). The suggestion panel never takes keyboard focus, so
//! the app being edited keeps its key window. Instead, while the review is open, SuperEditr's
//! keyboard tap takes the panel's keys before they reach that app and passes them on to the panel
//! as `overlay:key` events. ↩ applies the fix and never reaches the app, so a chat app can't send
//! the message underneath.
//!
//! This module is the platform-neutral part: which keys the panel takes. The macOS tap lives in
//! `platform::macos`.

use serde::Serialize;

/// One key press, named and flagged as a web page's `KeyboardEvent` names it, so the panel handles
/// it like a key typed into its own window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Key {
    /// "Enter", "Escape", "Tab", "ArrowLeft", " ", "Backspace", or the character typed ("a", "/").
    pub key: String,
    pub meta_key: bool,
    pub ctrl_key: bool,
    pub alt_key: bool,
    pub shift_key: bool,
}

/// Shortcuts with ⌘ or ⌃ that the panel uses: switching style (1–9), copying (C), trying again (R)
/// and Settings (,).
const PANEL_SHORTCUTS: [&str; 12] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "c", "r", ","];

impl Key {
    /// Whether the panel takes this key from the app. Every key that types or edits text does, as
    /// when the panel had keyboard focus, so nothing lands in the selection under review. So do ↩
    /// and esc with any modifier: ⌘↩ sends the message in many chat apps. Other shortcuts with ⌘
    /// or ⌃ still reach the app.
    pub fn for_panel(&self) -> bool {
        if self.key == "Enter" || self.key == "Escape" {
            true
        } else if self.meta_key || self.ctrl_key {
            PANEL_SHORTCUTS.contains(&self.key.to_lowercase().as_str())
        } else {
            !self.key.is_empty()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str, meta_key: bool, ctrl_key: bool) -> Key {
        Key { key: name.into(), meta_key, ctrl_key, alt_key: false, shift_key: false }
    }

    #[test]
    fn the_panel_takes_its_keys_and_typing() {
        for name in ["Enter", "Escape", "Tab", "ArrowDown", " ", "Backspace", "/", "a", "r", "3", "x"] {
            assert!(key(name, false, false).for_panel(), "{name}");
        }
        assert!(Key { alt_key: true, ..key("Enter", false, false) }.for_panel(), "⌥↩ applies too");
    }

    #[test]
    fn only_the_panels_own_shortcuts_are_kept_from_the_app() {
        for name in ["1", "9", "c", "C", "r", ","] {
            assert!(key(name, true, false).for_panel(), "⌘{name}");
        }
        assert!(key("2", false, true).for_panel(), "⌃2");
        assert!(key("Enter", true, false).for_panel(), "⌘↩ would send the message in a chat app");
        assert!(key("Enter", false, true).for_panel(), "⌃↩");
        for name in ["v", "z", "q", "w", "Tab"] {
            assert!(!key(name, true, false).for_panel(), "⌘{name} reaches the app");
        }
    }

    #[test]
    fn a_key_with_no_name_reaches_the_app() {
        assert!(!key("", false, false).for_panel());
    }
}
