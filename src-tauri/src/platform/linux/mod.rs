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
pub mod setup;
#[cfg(test)]
mod testing;

pub use log::{log_path, set_trace, trace};

use super::{PlatformError, Rect, ReplaceMethod, ReplaceOutcome};
use desktop::hyprland::HyprlandDesktop;
use desktop::shortcuts::HyprlandShortcuts;
use desktop::{Action, Binding, Desktop, FallbackDesktop, SetupCheck, ShortcutBackend, WindowRef};
use serde::Serialize;
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
            code: field.code.clone(),
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
    tauri::async_runtime::spawn(quit_on_signal());
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

/// Quits as the tray's Quit does when the session ends or something stops SuperEditr (SIGTERM,
/// SIGINT, SIGHUP): its binds and the panel's rule stay in the compositor until taken out.
async fn quit_on_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut interrupt), Ok(mut hangup)) =
        (signal(SignalKind::terminate()), signal(SignalKind::interrupt()), signal(SignalKind::hangup()))
    else {
        return;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = interrupt.recv() => {}
        _ = hangup.recv() => {}
    }
    stop();
    std::process::exit(0);
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

/// The Linux setup page: what SuperEditr found, what Set up Omarchy would change, and which running
/// apps it can't read until they restart.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupReport {
    /// Such as "Omarchy 4.0.6 on Hyprland 0.56.2".
    pub desktop: String,
    pub omarchy: bool,
    pub checks: Vec<SetupCheck>,
    pub changes: Vec<setup::Change>,
    pub restart: Vec<RestartApp>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RestartApp {
    pub name: String,
    pub reason: String,
}

/// Omarchy's version, when this is Omarchy.
fn omarchy_version() -> Option<String> {
    if !std::path::Path::new("/usr/share/omarchy").is_dir() {
        return None;
    }
    let version = std::process::Command::new("pacman")
        .args(["-Q", "omarchy"])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|line| line.split_whitespace().nth(1).map(|v| v.split('-').next().unwrap_or(v).to_string()));
    Some(version.unwrap_or_default())
}

pub async fn setup_report() -> SetupReport {
    let config = setup::config_home().unwrap_or_default();
    let desktop = desktop();
    let omarchy = omarchy_version();
    let mut checks = desktop.setup_status();
    let compositor =
        checks.first().map(|check| format!("{} {}", check.name, check.detail.split(',').next().unwrap_or("")));
    let name = match (&omarchy, compositor) {
        (Some(version), Some(compositor)) => format!("Omarchy {version} on {compositor}").replace("  ", " "),
        (None, Some(compositor)) => compositor,
        (_, None) => desktop.name().to_string(),
    };

    let limit = std::time::Duration::from_secs(2);
    let enabled = tokio::time::timeout(limit, ::atspi::connection::read_session_accessibility()).await;
    let registered = tokio::time::timeout(limit, ATSPI.registered_pids()).await;
    checks.push(SetupCheck {
        name: "Accessibility".into(),
        ok: matches!(enabled, Ok(Ok(true))) && matches!(registered, Ok(Ok(_))),
        detail: match (&enabled, &registered) {
            (Ok(Ok(true)), Ok(Ok(_))) => "On for this session, so apps opened from now on expose their text".into(),
            (Ok(Ok(false)), _) => "Off for this session. Quit and reopen SuperEditr, which turns it on.".into(),
            (Ok(Err(error)), _) => format!("The accessibility bus didn’t answer ({error}). Is at-spi2-core installed?"),
            (_, Ok(Err(error))) => format!("The accessibility registry didn’t answer ({error})"),
            _ => "The accessibility bus didn’t answer in time".into(),
        },
    });
    checks.push(clipboard_check());
    checks.push(setup::flags_check(&config));
    checks.push(setup::autostart_check(&config));
    checks.push(keyring_check().await);
    if let Some(shortcuts) = SHORTCUTS.get() {
        checks.push(shortcuts.status());
    }
    if std::path::Path::new("/proc/driver/nvidia").exists() {
        let set = std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER").is_ok_and(|v| v == "1");
        checks.push(SetupCheck {
            name: "NVIDIA".into(),
            ok: set,
            detail: if set {
                "WEBKIT_DISABLE_DMABUF_RENDERER=1 is set".into()
            } else {
                "If SuperEditr’s windows draw blank, add WEBKIT_DISABLE_DMABUF_RENDERER=1 to a file in ~/.config/uwsm/env.d/ and log in again.".into()
            },
        });
    }

    let mut changes = setup::flag_changes(&config);
    let autostart = setup::autostart_check(&config);
    changes.push(setup::Change {
        path: setup::tilde(&config.join(setup::AUTOSTART)),
        what: if autostart.ok {
            "SuperEditr already opens at login.".into()
        } else {
            "Open SuperEditr at login.".into()
        },
        needed: !autostart.ok,
    });

    let registered = match registered {
        Ok(Ok(pids)) => Some(pids),
        _ => None,
    };
    SetupReport {
        desktop: name,
        omarchy: omarchy.is_some(),
        checks,
        changes,
        restart: needs_restart(registered.as_deref()),
    }
}

/// The running apps SuperEditr can't read until they restart: Chromium and Electron apps started
/// without the flag, and apps started before accessibility was on (not on the accessibility bus).
fn needs_restart(registered: Option<&[i32]>) -> Vec<RestartApp> {
    let Some(windows) = hyprland().and_then(|h| h.windows().ok()) else { return Vec::new() };
    let mut seen = Vec::new();
    let mut apps = Vec::new();
    for window in windows {
        if window.pid == own_pid() || seen.contains(&window.pid) {
            continue;
        }
        seen.push(window.pid);
        let profile = apps::profile(&window.app_id, &window.tags);
        if profile.terminal || profile.password_manager {
            continue;
        }
        let reason = if setup::chromium_based(window.pid) && !setup::started_with_flag(window.pid) {
            format!("started without {}", setup::FLAG)
        } else if registered.is_some_and(|pids| !pids.contains(&window.pid)) {
            "started before accessibility was on".to_string()
        } else {
            continue;
        };
        apps.push(RestartApp { name: apps::display_name(&window.app_id), reason });
    }
    apps
}

/// Whether the clipboard can be read and restored without focus (the data-control protocol).
fn clipboard_check() -> SetupCheck {
    use wl_clipboard_rs::paste::{ClipboardType, Error, Seat, get_mime_types};
    let (ok, detail) = match get_mime_types(ClipboardType::Regular, Seat::Unspecified) {
        Err(Error::MissingProtocol { name, .. }) => {
            (false, format!("The desktop doesn’t offer {name}, so SuperEditr can’t copy or restore the clipboard"))
        }
        Err(Error::WaylandConnection(error)) => (false, format!("Couldn’t connect to the desktop: {error}")),
        _ => (true, "Read and restored through data-control".into()),
    };
    SetupCheck { name: "Clipboard".into(), ok, detail }
}

/// Whether a Secret Service keeps API keys, and whether its keyring has a password.
async fn keyring_check() -> SetupCheck {
    let owner = async {
        let connection = zbus::Connection::session().await.ok()?;
        let dbus = zbus::fdo::DBusProxy::new(&connection).await.ok()?;
        let name = zbus::names::BusName::try_from("org.freedesktop.secrets").ok()?;
        dbus.name_has_owner(name).await.ok()
    };
    let running = tokio::time::timeout(std::time::Duration::from_secs(2), owner).await.ok().flatten().unwrap_or(false);
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share")));
    let plain = data.map(|data| setup::plain_keyrings(&data)).unwrap_or_default();
    let detail = match (running, plain.is_empty()) {
        (false, _) => "No Secret Service is running, so API keys can’t be saved. Install gnome-keyring.".to_string(),
        (true, true) => "API keys are saved in your keyring".to_string(),
        (true, false) => format!(
            "API keys are saved in your keyring, but {} has no password (Omarchy’s default), so any program you run can read them. Seahorse (Passwords and Keys) can set one.",
            plain.join(", ")
        ),
    };
    SetupCheck { name: "Keyring".into(), ok: running, detail }
}
