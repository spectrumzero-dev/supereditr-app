//! Placeholder for platforms without an integration yet. On Windows, this interface could be
//! implemented with UI Automation (TextPattern2 for selection and bounds, ValuePattern or
//! SendInput Ctrl+V for replacement, and a WinEvent hook for selection changes).

use super::{PlatformError, Rect, ReplaceMethod, WatchEvent};

#[derive(Debug, Clone)]
pub struct Capture {
    pub pid: i32,
    pub bundle_id: String,
    pub app_name: String,
    pub text: String,
    pub bounds: Option<Rect>,
    pub editable: bool,
    pub playground: bool,
    pub code: Vec<String>,
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

pub fn accessibility_trusted(_prompt: bool) -> bool {
    false
}

pub fn open_accessibility_settings() {}

pub enum SettingsWindow {
    Front(Rect, i64),
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

pub fn take_review_keys(_take: bool) -> bool {
    false
}

pub fn return_focus(_pid: i32) {}

pub fn refresh_keyboard_layout() {}

pub fn own_pid() -> i32 {
    std::process::id() as i32
}

pub fn capture(_allow_clipboard: bool, _select_sentence: bool) -> Result<Capture, PlatformError> {
    Err(PlatformError::Other("This platform isn’t supported yet.".into()))
}

pub fn set_trace(_on: bool) {}

pub fn trace(_message: std::fmt::Arguments) {}

pub fn focused_app_identity() -> Option<(i32, String, String)> {
    None
}

pub fn selection_unchanged(_capture: &Capture) -> bool {
    false
}

pub fn replace(_capture: &Capture, _text: &str) -> Result<ReplaceMethod, PlatformError> {
    Err(PlatformError::ReplaceFailed)
}

pub fn copy_to_clipboard(_text: &str) {}

pub fn mouse_location() -> (f64, f64) {
    (0.0, 0.0)
}

pub fn start_app_switch_watcher(_on_switch: impl Fn(i32) + 'static) {}

pub fn start_selection_watcher(_on_event: impl Fn(WatchEvent) + Send + Sync + 'static) {}

pub fn selection_app_switched() {}
