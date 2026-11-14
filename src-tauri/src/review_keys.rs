//! Keys for a review the user opened (⌘E E). The suggestion panel never takes keyboard focus, so
//! the app being edited keeps its key window. Instead, while the review is open, SuperEditr's
//! keyboard tap takes the panel's keys before they reach that app and passes them on to the panel
//! as `overlay:key` events. ↩ applies the fix and never reaches the app, so a chat app can't send
//! the message underneath.
//!
//! A suggestion that opened by itself leaves the keys with the app, all but one: plain esc closes the
//! panel and never reaches the app. The user may have selected the text for something else, and esc
//! in the app can drop the selection or cancel what they were doing.
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

/// Shortcuts with ⌘ or ⌃ alone that the panel uses: switching style (1–9), copying (C), trying again (R)
/// and Settings (,).
const PANEL_SHORTCUTS: [&str; 12] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "c", "r", ","];

impl Key {
    /// Whether the panel takes this key from the app. Every key that types or edits text does, as
    /// when the panel had keyboard focus, so nothing lands in the selection under review. So do ↩
    /// and esc with any modifier: ⌘↩ sends the message in many chat apps. Other shortcuts with ⌘
    /// or ⌃ still reach the app, and whatever else answers to them.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn for_panel(&self) -> bool {
        if self.key == "Enter" || self.key == "Escape" {
            true
        } else if self.meta_key || self.ctrl_key {
            // ⌘ or ⌃ alone. With another modifier it is some other shortcut, such as a screenshot
            // tool's ⌘⇧4, and not the panel's ⌘4.
            let alone = self.meta_key != self.ctrl_key && !self.shift_key && !self.alt_key;
            alone && PANEL_SHORTCUTS.contains(&self.key.to_lowercase().as_str())
        } else {
            !self.key.is_empty()
        }
    }

    /// Whether this is the one key the panel takes from the app while a suggestion that opened by
    /// itself shows: esc alone. With a modifier it is some other shortcut, and the app's.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn closes_panel(&self) -> bool {
        self.key == "Escape" && !(self.meta_key || self.ctrl_key || self.alt_key || self.shift_key)
    }
}

/// Whether something other than the app under review has the keyboard: a launcher's search box, or
/// another panel that takes keys without coming to the front. The app in front doesn't change then,
/// so the review stays open, but the keys are for that box and not for the panel. Each argument is
/// a process ID, or 0 for none or not known: a notice belongs to no app, and until Accessibility
/// has said who has the keyboard the panel takes its keys, so ↩ never reaches a chat app by default.
/// SuperEditr's own windows never count as elsewhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn keyboard_is_elsewhere(reviewed: i32, keyboard: i32, own: i32) -> bool {
    reviewed != 0 && keyboard != 0 && keyboard != reviewed && keyboard != own
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
    fn a_suggestion_that_opened_by_itself_takes_only_plain_esc() {
        assert!(key("Escape", false, false).closes_panel());
        for name in ["Enter", "Tab", " ", "Backspace", "a", "/"] {
            assert!(!key(name, false, false).closes_panel(), "{name} reaches the app");
        }
        assert!(!key("Escape", true, false).closes_panel(), "⌘esc is the app's");
        assert!(!key("Escape", false, true).closes_panel(), "⌃esc is the app's");
        assert!(!Key { shift_key: true, ..key("Escape", false, false) }.closes_panel(), "⇧esc is the app's");
        assert!(!Key { alt_key: true, ..key("Escape", false, false) }.closes_panel(), "⌥esc has its own shortcut");
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
    fn the_panel_gives_up_its_keys_only_to_another_app_known_to_have_the_keyboard() {
        let (reviewed, launcher, own) = (100, 200, 300);
        assert!(keyboard_is_elsewhere(reviewed, launcher, own), "a launcher's box has the keyboard");
        assert!(!keyboard_is_elsewhere(reviewed, reviewed, own), "the app under review has it");
        assert!(!keyboard_is_elsewhere(reviewed, 0, own), "not known yet, or Accessibility was slow to say");
        assert!(!keyboard_is_elsewhere(reviewed, own, own), "the panel itself, while an instruction is typed");
        assert!(!keyboard_is_elsewhere(0, launcher, own), "a notice belongs to no app");
    }

    #[test]
    fn a_panel_shortcut_with_another_modifier_is_some_other_shortcut() {
        for name in ["4", "c", "r", ","] {
            assert!(!Key { shift_key: true, ..key(name, true, false) }.for_panel(), "⌘⇧{name} reaches the app");
            assert!(!Key { alt_key: true, ..key(name, true, false) }.for_panel(), "⌘⌥{name} reaches the app");
            assert!(!Key { shift_key: true, ..key(name, false, true) }.for_panel(), "⌃⇧{name} reaches the app");
            assert!(!key(name, true, true).for_panel(), "⌘⌃{name} reaches the app");
        }
    }

    #[test]
    fn a_key_with_no_name_reaches_the_app() {
        assert!(!key("", false, false).for_panel());
    }
}
