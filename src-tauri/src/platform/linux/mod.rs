//! Linux: the platform interface for Wayland desktops.
//!
//! Work that is the same on any Wayland desktop (accessibility, the clipboard, app profiles, the
//! log) lives here. What only the compositor can do (window identity, focus, placing the panel,
//! sending keys) goes through the [`desktop::Desktop`] chosen at startup, and shortcuts through a
//! [`desktop::ShortcutBackend`].
//!
//! A review reads the selection through AT-SPI, or with a guarded copy on a shortcut press
//! (`capture`), and writes a correction back only by pasting it into a field AT-SPI confirmed
//! (`replace`). The panel takes keyboard focus for a review started with the shortcut, and gives it
//! back to the app when it closes.

mod apps;
mod atspi;
mod capture;
mod clipboard;
pub mod desktop;
mod log;
mod replace;
#[cfg(test)]
mod testing;

pub use log::{log_path, set_trace, trace};

use super::{PlatformError, Rect, ReplaceMethod, ReplaceOutcome};
use desktop::hyprland::HyprlandDesktop;
use desktop::shortcuts::HyprlandShortcuts;
use desktop::{Action, Binding, Desktop, FallbackDesktop, ShortcutBackend, WindowRef};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

/// Where a capture's text came from. Only an AT-SPI capture can be editable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Atspi,
    Copy,
    /// Settings' "Try it now" box.
    Playground,
}

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
    pub source: Source,
    /// The window the text came from, which a replacement goes back to.
    pub window: Option<WindowRef>,
    /// The field, as read, which a replacement checks hasn't changed.
    pub field: Option<atspi::Field>,
}

impl Capture {
    pub fn protected(&self) -> Vec<(usize, usize)> {
        self.code.clone()
    }

    /// Text read through AT-SPI: editable when the field is and the app takes pastes.
    fn read(window: WindowRef, field: atspi::Field, profile: apps::Profile, bounds: Option<Rect>) -> Self {
        Self {
            pid: window.pid,
            bundle_id: window.app_id.clone(),
            app_name: apps::display_name(&window.app_id),
            text: field.selected.clone(),
            bounds,
            editable: field.editable && profile.write == apps::Write::Paste,
            playground: false,
            code: Vec::new(),
            source: Source::Atspi,
            window: Some(window),
            field: Some(field),
        }
    }

    /// Text from a guarded copy: never editable.
    fn copied(window: WindowRef, text: String, bounds: Option<Rect>) -> Self {
        Self {
            pid: window.pid,
            bundle_id: window.app_id.clone(),
            app_name: apps::display_name(&window.app_id),
            text,
            bounds,
            editable: false,
            playground: false,
            code: Vec::new(),
            source: Source::Copy,
            window: Some(window),
            field: None,
        }
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
        source: Source::Playground,
        window: None,
        field: None,
    }
}

/// Hyprland, when this session runs it.
fn hyprland() -> Option<Arc<HyprlandDesktop>> {
    static HYPRLAND: OnceLock<Option<Arc<HyprlandDesktop>>> = OnceLock::new();
    HYPRLAND.get_or_init(|| HyprlandDesktop::detect().map(Arc::new)).clone()
}

fn desktop() -> Arc<dyn Desktop> {
    match hyprland() {
        Some(hyprland) => hyprland,
        None => Arc::new(FallbackDesktop),
    }
}

static ATSPI: LazyLock<atspi::Atspi> = LazyLock::new(atspi::Atspi::default);
static SHORTCUTS: OnceLock<Arc<HyprlandShortcuts>> = OnceLock::new();
/// The window that was in front when the latest review began, which the panel gives focus back to.
static TARGET: Mutex<Option<WindowRef>> = Mutex::new(None);
/// Held while a capture or replacement runs: each one is a clipboard transaction, and two at once
/// would each take the other's copy for their own.
static CHAIN: Mutex<()> = Mutex::new(());

/// At launch: turns accessibility on for the session, so Qt, Firefox, Chromium and Electron apps
/// opened from now on expose their text (it's off on a fresh Omarchy, and nothing makes it stick
/// across logins), follows focus through AT-SPI, and sets up the desktop's events and shortcuts.
pub fn start(on_shortcut: impl Fn(Action) + Send + Sync + 'static) {
    tauri::async_runtime::spawn(async {
        if let Err(error) = ::atspi::connection::set_session_accessibility(true).await {
            eprintln!("SuperEditr: couldn’t turn on accessibility for this session: {error}");
        }
        if let Err(error) = ATSPI.follow_focus().await {
            trace(format_args!("AT-SPI focus events stopped: {error}"));
        }
    });
    match hyprland() {
        Some(hyprland) => {
            for check in hyprland.setup_status() {
                trace(format_args!("{} {}: {}", hyprland.name(), check.name, check.detail));
                if !check.ok {
                    eprintln!("SuperEditr: {}: {}", check.name, check.detail);
                }
            }
            hyprland.follow_events();
            let shortcuts = HyprlandShortcuts::new(hyprland);
            let _ = shortcuts.listen(Box::new(on_shortcut));
            let _ = SHORTCUTS.set(shortcuts.clone());
            tauri::async_runtime::spawn(shortcuts.start());
        }
        None => trace(format_args!("desktop: not Hyprland; suggestions go to the clipboard")),
    }
}

/// Binds the review and quick-fix shortcuts. Returns what couldn't be bound.
pub fn set_shortcuts(bindings: &[Binding]) -> Vec<String> {
    match SHORTCUTS.get() {
        Some(shortcuts) => shortcuts.install(bindings),
        None if bindings.is_empty() => Vec::new(),
        None => vec!["SuperEditr can bind shortcuts only on Hyprland for now. Bind a key to `supereditr review` in your desktop's settings.".into()],
    }
}

/// What in the desktop already uses this shortcut, if anything does.
pub fn shortcut_taken(accelerator: &str) -> Option<String> {
    let shortcut = desktop::Shortcut::parse(accelerator)?;
    SHORTCUTS.get()?.taken_by(&Binding { action: Action::Review, shortcut })
}

/// Takes SuperEditr's binds and window rule out of the compositor, when SuperEditr quits.
pub fn stop() {
    if let Some(shortcuts) = SHORTCUTS.get() {
        shortcuts.uninstall();
    }
}

/// Calls `on_switch` with the pid of each window that comes to the front.
pub fn start_app_switch_watcher(on_switch: impl Fn(i32) + Send + Sync + 'static) {
    std::mem::forget(desktop().watch_front_window(Box::new(move |window| on_switch(window.pid))));
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

/// Gives keyboard focus back to the window the review started in, if it's still `pid`'s.
pub fn return_focus(pid: i32) {
    let target = TARGET.lock().unwrap().clone();
    if let Some(window) = target.filter(|window| window.pid == pid) {
        let desktop = desktop();
        tauri::async_runtime::spawn(async move {
            let _ = desktop.focus(&window).await;
        });
    }
}

pub fn own_pid() -> i32 {
    std::process::id() as i32
}

fn services<'a>(desktop: &'a dyn Desktop, clipboard: &'a clipboard::Wayland) -> capture::Services<'a> {
    capture::Services { desktop, fields: &*ATSPI, clipboard }
}

/// Reads the selection in the window in front: through AT-SPI, or, with `allow_clipboard` (a
/// shortcut press), a guarded copy. With `select_sentence` and nothing selected, first selects the
/// sentence before the caret. Runs on a worker thread.
pub fn capture(allow_clipboard: bool, select_sentence: bool) -> Result<Capture, PlatformError> {
    let _one_at_a_time = CHAIN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let desktop = desktop();
    *TARGET.lock().unwrap() = desktop.front_window().ok().filter(|window| window.pid != own_pid());
    let clipboard = clipboard::Wayland;
    tauri::async_runtime::block_on(capture::capture(
        &services(desktop.as_ref(), &clipboard),
        allow_clipboard,
        select_sentence,
    ))
}

pub fn focused_app_identity() -> Option<(i32, String, String)> {
    let window = desktop().front_window().ok()?;
    let name = apps::display_name(&window.app_id);
    Some((window.pid, window.app_id, name))
}

/// Pastes `text` over the captured selection. `_proofread` asks macOS to change only the words a
/// proofread fixes; Linux always replaces the whole selection. Runs on a worker thread.
pub fn replace(capture: &Capture, text: &str, _proofread: bool) -> Result<ReplaceMethod, PlatformError> {
    let _one_at_a_time = CHAIN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let desktop = desktop();
    let clipboard = clipboard::Wayland;
    match tauri::async_runtime::block_on(replace::replace(&services(desktop.as_ref(), &clipboard), capture, text)) {
        ReplaceOutcome::Applied => Ok(ReplaceMethod::Paste),
        ReplaceOutcome::CopyOnly(error) => Err(error),
        ReplaceOutcome::Uncertain(_) => Err(PlatformError::Unverified),
    }
}

/// Puts a suggestion on the clipboard, marked sensitive so clipboard history skips it.
pub fn copy_to_clipboard(text: &str) {
    if let Err(error) = clipboard::copy_text(&clipboard::Wayland, text) {
        eprintln!("SuperEditr: couldn’t copy to the clipboard: {error}");
    }
}

pub fn mouse_location() -> (f64, f64) {
    desktop().cursor().unwrap_or((0.0, 0.0))
}

pub fn frontmost_pid() -> Option<i32> {
    desktop().front_window().ok().map(|window| window.pid)
}

pub fn app_identity(pid: i32) -> (String, String) {
    let window = hyprland().and_then(|h| h.windows().ok()).and_then(|all| all.into_iter().find(|w| w.pid == pid));
    match window {
        Some(window) => {
            let name = apps::display_name(&window.app_id);
            (window.app_id, name)
        }
        None => (String::new(), "this app".into()),
    }
}

pub fn selection_app_switched() {}

/// The panel's window title, which the desktop's rule for it matches.
pub const OVERLAY_TITLE: &str = desktop::lua::OVERLAY_TITLE;

/// Where the panel goes: where it opens next, or, while it shows, where it moves.
pub fn place_overlay(at: Rect, shown: bool) {
    if let Err(error) = desktop().place_overlay(at, shown) {
        trace(format_args!("panel placement: {error}"));
    }
}

/// Gives the panel keyboard focus once it's on screen.
pub fn focus_overlay() {
    let desktop = desktop();
    tauri::async_runtime::spawn(async move {
        let started = std::time::Instant::now();
        match desktop.focus_overlay().await {
            Ok(()) => trace(format_args!("panel focused after {:?}", started.elapsed())),
            Err(error) => trace(format_args!("panel focus: {error}")),
        }
    });
}

/// Whether the panel has keyboard focus now.
pub fn overlay_focused() -> bool {
    hyprland().is_some_and(|hyprland| hyprland.overlay_focused())
}

/// The panel closed while it had focus: focus goes back to the window the review started in.
pub fn overlay_closed() {
    // Not inside an `if let` on the lock: `return_focus` takes it too.
    let target = TARGET.lock().unwrap().clone();
    if let Some(window) = target {
        return_focus(window.pid);
    }
}
