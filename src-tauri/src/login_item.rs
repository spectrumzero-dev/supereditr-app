//! Opening at login. macOS and Linux use the autostart plugin, which writes a Launch Agent or an XDG
//! autostart entry. Windows writes its own Run value (`platform::login`).

use tauri::AppHandle;

#[cfg(not(windows))]
pub fn is_enabled(app: &AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn enable(app: &AppHandle) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().enable().map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn disable(app: &AppHandle) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().disable().map_err(|e| e.to_string())
}

#[cfg(windows)]
pub fn is_enabled(_app: &AppHandle) -> Result<bool, String> {
    crate::platform::login::is_enabled()
}

#[cfg(windows)]
pub fn enable(_app: &AppHandle) -> Result<(), String> {
    crate::platform::login::enable()
}

#[cfg(windows)]
pub fn disable(_app: &AppHandle) -> Result<(), String> {
    crate::platform::login::disable()
}
