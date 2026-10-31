//! Linux: the platform interface for Wayland desktops.
//!
//! Work that is the same on any Wayland desktop (accessibility, the clipboard, app profiles, the
//! log) lives here. What only the compositor can do (window identity, focus, placing the panel,
//! sending keys) goes through the [`desktop::Desktop`] chosen at startup, and shortcuts through a
//! [`desktop::ShortcutBackend`].
//!
//! Reading and replacing text in other apps isn't implemented yet: `capture` and `replace` refuse,
//! and Settings and its "Try it now" box work on their own.

pub mod desktop;
mod log;

pub use log::{log_path, set_trace, trace};

use super::{PlatformError, Rect, ReplaceMethod};

#[derive(Debug, Clone)]
pub struct Capture {
    pub pid: i32,
    /// The Wayland app ID (Hyprland's window class), for example "foot" or "chromium".
    pub bundle_id: String,
    pub app_name: String,
    pub text: String,
    pub bounds: Option<Rect>,
    pub editable: bool,
    pub playground: bool,
    pub code: Vec<(usize, usize)>,
}

impl Capture {
    pub fn protected(&self) -> Vec<(usize, usize)> {
        self.code.clone()
    }
}

pub fn playground_capture(text: String, bounds: Rect) -> Capture {
    Capture {
        pid: own_pid(),
        bundle_id: String::new(),
        app_name: "SuperEditr".into(),
        text,
        bounds: Some(bounds),
        editable: true,
        playground: true,
        code: Vec::new(),
    }
}

/// Linux has no permission like macOS's Accessibility switch: any app in the session may use the
/// accessibility bus.
pub fn accessibility_trusted(_prompt: bool) -> bool {
    true
}

pub fn open_accessibility_settings() {}

/// System Settings is macOS's; on Linux it's always reported closed.
pub enum SettingsWindow {
    #[allow(dead_code)]
    Front(Rect, i64),
    #[allow(dead_code)]
    Behind,
    Closed,
}

pub fn system_settings_window() -> SettingsWindow {
    SettingsWindow::Closed
}

pub fn own_bundle() -> Option<std::path::PathBuf> {
    None
}

pub fn set_double_presses(_chords: Vec<crate::double_press::Chord>) {}

pub fn pause_double_presses(_paused: bool) {}

pub fn start_double_press_tap(
    _on_fire: impl Fn(usize) + Send + Sync + 'static,
    _on_review_key: impl Fn(crate::review_keys::Key) + Send + Sync + 'static,
) {
}

/// Wayland gives no app the keyboard of another, so the panel takes focus for its keys.
pub fn take_review_keys(_take: bool) -> bool {
    false
}

pub fn return_focus(_pid: i32) {}

pub fn own_pid() -> i32 {
    std::process::id() as i32
}

pub fn capture(_allow_clipboard: bool, _select_sentence: bool) -> Result<Capture, PlatformError> {
    Err(PlatformError::Other("SuperEditr can’t read text in other apps on Linux yet.".into()))
}

pub fn focused_app_identity() -> Option<(i32, String, String)> {
    None
}

pub fn replace(_capture: &Capture, _text: &str, _proofread: bool) -> Result<ReplaceMethod, PlatformError> {
    Err(PlatformError::ReplaceFailed)
}

pub fn copy_to_clipboard(_text: &str) {}

pub fn mouse_location() -> (f64, f64) {
    (0.0, 0.0)
}

pub fn frontmost_pid() -> Option<i32> {
    None
}

pub fn app_identity(_pid: i32) -> (String, String) {
    (String::new(), "this app".into())
}

pub fn selection_app_switched() {}
