//! macOS integration through the Accessibility (AX) API, Core Graphics keyboard events, and
//! NSPasteboard.
//!
//! Every AX call uses a short messaging timeout and runs off the main thread, so an
//! unresponsive app can delay a review by a fraction of a second but never hang SuperEditr.

#![allow(non_upper_case_globals, non_snake_case)]

mod apps;
mod ax;
mod capture;
mod glide;
mod keys;
mod log;
mod pasteboard;
mod replace;
mod seam;
mod settings_window;
#[cfg(test)]
mod testing;
mod text;
mod watch;
mod window;

pub use apps::{app_identity, focused_app_identity, frontmost_pid, return_focus, return_to_field};
pub use ax::Element;
pub use capture::capture;
pub use glide::{glide_own_window, stop_own_window_glide};
pub use keys::{
    pause_double_presses, refresh_keyboard_layout, set_double_presses, set_review_app, start_double_press_tap,
    take_escape_key, take_review_keys,
};
pub use log::{log_path, set_trace, trace};
pub use replace::{copy_to_clipboard, replace};
pub use settings_window::{SettingsWindow, own_bundle, system_settings_window};
pub use watch::{
    mouse_button_down, mouse_location, pressed_on_window, selection_app_switched, start_app_switch_watcher,
    start_selection_watcher, start_space_change_watcher,
};
pub use window::{WindowWatch, watch_window};

use super::Rect;
use ax::{AXIsProcessTrusted, AXIsProcessTrustedWithOptions, kAXTrustedCheckOptionPrompt};
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;

#[derive(Debug, Clone)]
pub struct Capture {
    pub pid: i32,
    pub bundle_id: String,
    pub app_name: String,
    pub text: String,
    pub bounds: Option<Rect>,
    /// The focused element is a text input (not static text on a page).
    pub editable: bool,
    /// Inside web content or an Electron app: replace by pasting so the page's editor sees it.
    web: bool,
    element: Option<Element>,
    range: Option<(isize, isize)>,
    /// Text from Settings' "Try it now" box: applying sends it back to the box, not to an app.
    pub playground: bool,
    /// Character ranges of `text` the app formats as code, whose backticks are gone from `text`:
    /// reviews leave them as written.
    pub code: Vec<(usize, usize)>,
    /// Character ranges of `text` that are list markers ("• ", "1. "): reviews leave them alone.
    markers: Vec<(usize, usize)>,
    /// A web field with formatting (code, a link, a list or a heading) that replacing the whole
    /// selection could lose, so a proofread there is worth applying in place.
    formatted: bool,
}

impl Capture {
    /// Ranges of `text` a review leaves as written: code, and list markers.
    pub fn protected(&self) -> Vec<(usize, usize)> {
        supereditr_core::policy::protected_ranges(&self.code, &self.markers)
    }

    /// Whether `other` was read from the same place as this capture, as far as Accessibility can
    /// tell: the same element, the same range in it, read the same way. Two captures that fell back
    /// to copying have no element to compare.
    pub fn same_field(&self, other: &Capture) -> bool {
        let same_element = match (&self.element, &other.element) {
            (Some(a), Some(b)) => a.same_as(b),
            (None, None) => true,
            _ => false,
        };
        same_element
            && self.range == other.range
            && self.web == other.web
            && self.markers == other.markers
            && self.formatted == other.formatted
    }

    /// How to make a copy-only capture replaceable. macOS has no such case.
    pub fn copy_only_hint(&self) -> Option<String> {
        None
    }
}

/// A capture of text in Settings' "Try it now" box, anchored at `bounds` (screen points).
pub fn playground_capture(text: String, bounds: Rect) -> Capture {
    Capture {
        pid: own_pid(),
        bundle_id: app_identity(own_pid()).0,
        app_name: "SuperEditr".into(),
        text,
        bounds: Some(bounds),
        editable: true,
        web: false,
        element: None,
        range: None,
        playground: true,
        code: Vec::new(),
        markers: Vec::new(),
        formatted: false,
    }
}

pub fn own_pid() -> i32 {
    std::process::id() as i32
}

pub fn accessibility_trusted(prompt: bool) -> bool {
    if !prompt {
        return unsafe { AXIsProcessTrusted() };
    }
    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), CFBoolean::true_value().as_CFType())]);
    unsafe { AXIsProcessTrustedWithOptions(options.as_CFTypeRef()) }
}

pub fn open_accessibility_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}

/// A mode of the program that runs instead of the app, named by its first argument. macOS has none.
pub fn run_helper_command(_args: &[String]) -> Option<i32> {
    None
}

/// SuperEditr is a menu-bar app, out of the Dock and ⌘Tab, except while Settings is open: then it
/// behaves like a normal app so the user can switch back to it.
pub fn set_in_dock(app: &tauri::AppHandle, in_dock: bool) {
    let _ = app.set_activation_policy(if in_dock {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    });
}

/// Whether a Launch Agent pointing at this executable keeps working after a restart: it's inside an
/// app bundle, and not on a mounted disk image or in the temporary copy macOS runs a downloaded app
/// from until it's moved (App Translocation).
pub fn lasting_app_path(exe: &str) -> bool {
    exe.contains(".app/Contents/MacOS/") && !exe.starts_with("/Volumes/") && !exe.contains("/AppTranslocation/")
}

/// After the login item is turned on. The Launch Agent needs nothing more.
pub fn reload_autostart() {}

/// Whether the desktop setup is done, so Settings needn't open at launch. macOS has no setup page.
pub fn desktop_set_up() -> bool {
    false
}

tauri_nspanel::tauri_panel! {
    panel!(OverlayPanel {
        config: {
            can_become_key_window: true,
            can_become_main_window: false,
            is_floating_panel: true,
            hides_on_deactivate: false
        }
    })
}

/// Converts the overlay window to a panel once at start-up.
pub fn init_overlay(window: &tauri::WebviewWindow) {
    use tauri_nspanel::{CollectionBehavior, PanelLevel, StyleMask, WebviewWindowExt};
    if let Ok(panel) = window.to_panel::<OverlayPanel>() {
        panel.set_level(PanelLevel::PopUpMenu.value());
        let _ = panel.set_style_mask(StyleMask::empty().borderless().nonactivating_panel().value());
        // It opens on the Space the user is on, and stays there: on a move to another Space it
        // slides away with the text it reviews, and closes once the move ends
        // (`review::on_space_change`). On every Space, it would float over the switch.
        panel.set_collection_behavior(
            CollectionBehavior::new().move_to_active_space().full_screen_auxiliary().transient().value(),
        );
        panel.set_has_shadow(false);
        panel.set_opaque(false);
    }
}

#[cfg(test)]
mod tests {
    use super::ax::AXUIElementCreateSystemWide;
    use super::*;

    #[test]
    fn the_same_field_has_the_same_selection_read_the_same_way() {
        let bounds = Rect { x: 10.0, y: 10.0, width: 300.0, height: 20.0 };
        let read = || playground_capture("Rhyan sent the KPI report.".into(), bounds);
        assert!(read().same_field(&read()));
        let mut moved = read();
        moved.range = Some((4, 26));
        assert!(!read().same_field(&moved), "another range");
        let mut web = read();
        web.web = true;
        assert!(!read().same_field(&web), "read from a web page");
        let mut formatted = read();
        formatted.formatted = true;
        assert!(!read().same_field(&formatted), "formatting around it");
        let mut element = read();
        element.element = Element::from_create(unsafe { AXUIElementCreateSystemWide() });
        assert!(!read().same_field(&element), "an element against none");
        assert!(element.same_field(&element.clone()), "the same element");
    }
}
