//! Updates for official release builds. The app asks SuperEditr's update server for `latest.json`
//! at `https://updates.supereditr.app/{{target}}/{{arch}}/{{current_version}}`, for example
//! `/darwin/aarch64/0.1.0`. Release builds set that endpoint at build time, in place of the GitHub
//! URL in tauri.conf.json. The server counts the check by those three values, stores no IP address,
//! and redirects to `latest.json` on the public repository's newest GitHub Release; the app sends no
//! identifier. The app checks the download against SuperEditr's own updater key (the public half is
//! in tauri.conf.json) and installs it in place; the new version runs after a restart.
//!
//! Only official release builds check for updates: they are compiled with
//! `SUPEREDITR_RELEASE_BUILD` set. Source builds and test builds never replace themselves, because an
//! official build is signed differently and macOS would ask for Accessibility permission again. The
//! one exception is the UI smoke tests, which give a copy of a test build a feed and key of their own.
//!
//! Choosing Check for Updates… opens a small window at once, which shows how the check goes and what
//! to do next. Automatic checks open nothing, so they never interrupt typing: an update they install
//! waits in the menu-bar menu.
//!
//! On Windows, installing runs the downloaded installer and quits SuperEditr at once, and the
//! installer starts it again when it's done. So an update is downloaded as soon as it's found and
//! kept until the user chooses Restart to Update, or until SuperEditr is idle (`hold`), or until the
//! user quits it.

use semver::Version;
use serde::Serialize;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tauri::menu::MenuItem;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_updater::UpdaterExt;

/// Release builds update themselves on macOS and Windows. Linux has no in-app updater: the package
/// manager installs new versions.
pub const ENABLED: bool =
    updates_itself(option_env!("SUPEREDITR_RELEASE_BUILD").is_some(), cfg!(target_os = "macos"), cfg!(windows));

const fn updates_itself(release_build: bool, macos: bool, windows: bool) -> bool {
    release_build && (macos || windows)
}

/// How long after launch the first automatic check waits, and how often it repeats.
const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// Without limits a stalled request would never finish, and the check would never end.
const FEED_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How often a downloaded Windows update looks for a quiet moment to install, and how long the user
/// must have left the keyboard and mouse alone first.
#[cfg_attr(not(windows), allow(dead_code))]
const IDLE_POLL: Duration = Duration::from_secs(10);
#[cfg_attr(not(windows), allow(dead_code))]
const USER_IDLE: Duration = Duration::from_secs(60);

/// The update window's label, and the event that tells it what's happening.
const WINDOW: &str = "update";
const STATUS_EVENT: &str = "update:status";

/// Where updating has got to. Serialized for the update window (`UpdateStatus` in types.ts).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Status {
    Idle,
    Checking,
    Downloading {
        version: String,
        percent: Option<u8>,
    },
    /// Installed, waiting for a restart.
    Ready {
        version: String,
    },
    /// `version` is the one running.
    UpToDate {
        version: String,
    },
    CheckFailed,
    InstallFailed {
        version: String,
    },
}

impl Status {
    /// Whether a check can start: none is running, and no update is waiting for a restart.
    fn can_check(&self) -> bool {
        !matches!(self, Status::Checking | Status::Downloading { .. } | Status::Ready { .. })
    }

    /// The menu-bar menu's line. A window reports a result, so a check that found nothing or failed
    /// leaves the usual words behind.
    fn menu_line(&self) -> String {
        match self {
            Status::Idle | Status::UpToDate { .. } | Status::CheckFailed => "Check for Updates…".into(),
            Status::Checking => "Checking for Updates…".into(),
            Status::Downloading { version, .. } => format!("Downloading SuperEditr {version}…"),
            Status::Ready { version } => format!("Restart to Update to {version}"),
            Status::InstallFailed { .. } => "Update Failed. Try Again…".into(),
        }
    }
}

/// The menu-bar menu's update line, and where updating has got to.
pub struct UpdateItem {
    item: MenuItem<tauri::Wry>,
    status: Mutex<Status>,
    /// On Windows, the downloaded update, kept until it's installed.
    #[cfg(windows)]
    pending: Mutex<Option<(tauri_plugin_updater::Update, Vec<u8>)>>,
}

impl UpdateItem {
    fn set(&self, app: &AppHandle, status: Status) {
        *self.status.lock().unwrap() = status.clone();
        self.publish(app, &status);
    }

    /// Starts a check, unless there's nothing for one to do.
    fn begin(&self, app: &AppHandle) -> bool {
        {
            let mut status = self.status.lock().unwrap();
            if !status.can_check() {
                return false;
            }
            *status = Status::Checking;
        }
        self.publish(app, &Status::Checking);
        true
    }

    fn publish(&self, app: &AppHandle, status: &Status) {
        let _ = self.item.set_text(status.menu_line());
        let _ = app.emit_to(WINDOW, STATUS_EVENT, status);
    }
}

/// The feed and key the UI smoke tests give a test build. Official builds and Linux ignore them.
fn test_feed() -> Option<(String, String)> {
    if ENABLED || !cfg!(any(target_os = "macos", windows)) {
        return None;
    }
    Some((std::env::var("SUPEREDITR_TEST_UPDATE_FEED").ok()?, std::env::var("SUPEREDITR_TEST_UPDATE_KEY").ok()?))
}

/// Whether this copy of the app can update itself.
pub fn available() -> bool {
    ENABLED || test_feed().is_some()
}

/// Whether this copy checks for updates by itself. A Windows test build given a feed does too, so a
/// test can see an update arrive in the middle of a review.
pub fn checks_automatically() -> bool {
    ENABLED || (cfg!(windows) && test_feed().is_some())
}

/// How long after launch the first automatic check waits. A Windows test build can be given a
/// shorter wait (`SUPEREDITR_TEST_UPDATE_DELAY`, in seconds).
fn first_check() -> Duration {
    #[cfg(windows)]
    if let Some(seconds) =
        test_feed().and(std::env::var("SUPEREDITR_TEST_UPDATE_DELAY").ok()).and_then(|seconds| seconds.parse().ok())
    {
        return Duration::from_secs(seconds);
    }
    FIRST_CHECK
}

/// Starting a review and a quick fix count as busy: an update doesn't install in the middle of
/// one, while the clipboard may be put aside or a fix is about to be written.
static BUSY: AtomicUsize = AtomicUsize::new(0);

/// Keeps a Windows update from installing until it's dropped.
pub struct Busy(());

pub fn busy() -> Busy {
    BUSY.fetch_add(1, Ordering::SeqCst);
    Busy(())
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.fetch_sub(1, Ordering::SeqCst);
    }
}

/// What keeps a downloaded update from installing now. Installing quits SuperEditr and shows the
/// installer's progress window, so it waits until none of these holds.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(not(windows), allow(dead_code))]
enum Hold {
    /// A review is starting or a quick fix is running.
    Working,
    /// The panel shows a review or a message.
    Panel,
    /// Settings or another SuperEditr window is open, and may hold an unsaved edit.
    Window,
    /// The user typed or moved the mouse in the last minute, so the installer's window could catch
    /// them typing.
    UserActive,
}

#[cfg_attr(not(windows), allow(dead_code))]
fn hold(working: bool, panel: bool, window: bool, idle: Duration) -> Option<Hold> {
    if working {
        Some(Hold::Working)
    } else if panel {
        Some(Hold::Panel)
    } else if window {
        Some(Hold::Window)
    } else if idle < USER_IDLE {
        Some(Hold::UserActive)
    } else {
        None
    }
}

/// Gives a test build the real updater, with only its feed and key changed, and plain HTTP allowed
/// because the feed is on the same Mac.
pub fn use_test_feed(context: &mut tauri::Context<tauri::Wry>) {
    let Some((feed, key)) = test_feed() else { return };
    if let Some(updater) = context.config_mut().plugins.0.get_mut("updater") {
        updater["endpoints"] = serde_json::json!([feed]);
        updater["pubkey"] = key.into();
        updater["dangerousInsecureTransportProtocol"] = true.into();
    }
}

pub fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry, tauri_plugin_updater::Config> {
    tauri_plugin_updater::Builder::new().build()
}

// The menu items are for the menu-bar menu; Linux updates come from the package manager.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub fn menu_item(app: &AppHandle) -> tauri::Result<MenuItem<tauri::Wry>> {
    let status = Status::Idle;
    let item = MenuItem::with_id(app, "update", status.menu_line(), true, None::<&str>)?;
    app.manage(UpdateItem {
        item: item.clone(),
        status: Mutex::new(status),
        #[cfg(windows)]
        pending: Mutex::new(None),
    });
    Ok(item)
}

/// Checks soon after launch and then every few hours, without asking.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(first_check()).await;
        loop {
            if app.try_state::<UpdateItem>().is_some_and(|state| state.begin(&app)) {
                check(&app).await;
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// The menu item and the window's main button: restart into an installed update, or check now and
/// show how it goes. While a check is running, this just shows that check.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub fn on_menu(app: &AppHandle) {
    let Some(state) = app.try_state::<UpdateItem>() else { return };
    if matches!(status(app), Status::Ready { .. }) {
        #[cfg(not(windows))]
        app.restart();
        #[cfg(windows)]
        return install(app, true);
    }
    let started = state.begin(app);
    show_window(app);
    if started {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { check(&app).await });
    }
}

pub fn status(app: &AppHandle) -> Status {
    app.try_state::<UpdateItem>().map_or(Status::Idle, |state| state.status.lock().unwrap().clone())
}

/// Made when it's needed and gone when closed, so a menu-bar app doesn't keep a web view for it.
fn show_window(app: &AppHandle) {
    let window = app.get_webview_window(WINDOW).map(Ok).unwrap_or_else(|| {
        WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("update.html".into()))
            .title("SuperEditr Updates")
            .inner_size(420.0, 200.0)
            .resizable(false)
            .minimizable(false)
            .maximizable(false)
            .always_on_top(true)
            .center()
            .build()
    });
    match window {
        Ok(window) => {
            let _ = window.set_focus();
        }
        Err(error) => eprintln!("SuperEditr: couldn't open the update window: {error}"),
    }
}

/// Runs the check `UpdateItem::begin` started.
async fn check(app: &AppHandle) {
    let Some(state) = app.try_state::<UpdateItem>() else { return };
    let builder = app
        .updater_builder()
        .timeout(FEED_TIMEOUT)
        .version_comparator(|current, remote| should_install(&current, &remote.version));
    // The installer quits SuperEditr without its usual cleanup, which would leave the tray icon in the
    // notification area until the pointer passes over it.
    #[cfg(windows)]
    let builder = {
        let app = app.clone();
        builder.on_before_exit(move || drop(app.remove_tray_by_id("main")))
    };
    let updater = builder.build();
    let found = match updater {
        Ok(updater) => updater.check().await,
        Err(error) => Err(error),
    };
    let result = match found {
        Ok(Some(mut update)) => {
            // The feed's timeout doesn't carry over to the download.
            update.timeout = Some(DOWNLOAD_TIMEOUT);
            let version = update.version.clone();
            state.set(app, Status::Downloading { version: version.clone(), percent: None });
            let (mut received, mut shown) = (0, None);
            let progress = |chunk: usize, total: Option<u64>| {
                received += chunk as u64;
                let percent = percent_of(received, total);
                if percent != shown {
                    shown = percent;
                    state.set(app, Status::Downloading { version: version.clone(), percent });
                }
            };
            #[cfg(not(windows))]
            let installed = update.download_and_install(progress, || {}).await;
            // Installing quits SuperEditr, so the download waits for `install`.
            #[cfg(windows)]
            let installed = update.download(progress, || {}).await.map(|bytes| {
                crate::platform::trace(format_args!("update: downloaded {version}"));
                *state.pending.lock().unwrap() = Some((update.clone(), bytes));
            });
            match installed {
                Ok(()) => Status::Ready { version },
                Err(error) => {
                    eprintln!("SuperEditr: couldn't install update {version}: {error}");
                    Status::InstallFailed { version }
                }
            }
        }
        Ok(None) => Status::UpToDate { version: app.package_info().version.to_string() },
        Err(error) => {
            eprintln!("SuperEditr: couldn't check for updates: {error}");
            Status::CheckFailed
        }
    };
    #[cfg(windows)]
    let ready = matches!(result, Status::Ready { .. });
    state.set(app, result);
    #[cfg(windows)]
    if ready {
        install_when_idle(app);
    }
}

/// Waits for a quiet moment (`hold`), then installs the downloaded update. The installer starts
/// SuperEditr again when it's done.
#[cfg(windows)]
fn install_when_idle(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut said = None;
        let mut last = std::time::SystemTime::now();
        let mut woke = None;
        loop {
            tokio::time::sleep(IDLE_POLL).await;
            if !matches!(status(&app), Status::Ready { .. }) {
                return;
            }
            let at = std::time::SystemTime::now();
            if slept(at.duration_since(last).unwrap_or_default()) {
                crate::platform::trace(format_args!("update: the computer was asleep"));
                woke = Some(at);
            }
            last = at;
            let awake = woke.map_or(Duration::MAX, |woke| at.duration_since(woke).unwrap_or_default());
            let now = hold_now(&app, awake);
            if now != said {
                match now {
                    Some(reason) => crate::platform::trace(format_args!("update: waiting ({reason:?})")),
                    None => crate::platform::trace(format_args!("update: idle")),
                }
                said = now;
            }
            if now.is_none() {
                return install(&app, true);
            }
        }
    });
}

/// `awake` is how long since the computer last woke from sleep, if it slept while waiting. Windows may
/// not count waking as input, so the time asleep would look like the user being away from the
/// keyboard just as they come back to it.
#[cfg(windows)]
fn hold_now(app: &AppHandle, awake: Duration) -> Option<Hold> {
    use crate::state::AppState;
    let state = app.state::<AppState>();
    let panel = state.session.lock().unwrap().is_some() || state.notice_visible.load(Ordering::Relaxed);
    let window = app.webview_windows().values().any(|window| window.is_visible().unwrap_or(false));
    let idle = crate::platform::input_idle().min(awake);
    hold(BUSY.load(Ordering::SeqCst) > 0, panel, window, idle)
}

/// Whether the wall clock moved on much further than a poll's wait: the computer slept meanwhile.
#[cfg_attr(not(windows), allow(dead_code))]
fn slept(between_polls: Duration) -> bool {
    between_polls > IDLE_POLL * 3
}

/// Installs the downloaded update, if there is one, and starts SuperEditr again afterwards when
/// `restart` is set. Runs the installer and quits; returns only if there was nothing to install or
/// it couldn't start the installer.
#[cfg(windows)]
pub fn install(app: &AppHandle, restart: bool) {
    let Some(state) = app.try_state::<UpdateItem>() else { return };
    let Some((update, bytes)) = state.pending.lock().unwrap().take() else { return };
    let version = update.version.clone();
    crate::platform::trace(format_args!("update: installing {version}"));
    let update = update.restart_after_install(restart);
    // On the main thread, where the tray icon lives, so it can be taken away before the app quits.
    let main = app.clone();
    let started = app.run_on_main_thread(move || {
        if let Err(error) = update.install(&bytes) {
            eprintln!("SuperEditr: couldn't install update {version}: {error}");
            if let Some(state) = main.try_state::<UpdateItem>() {
                state.set(&main, Status::InstallFailed { version });
            }
        }
    });
    if let Err(error) = started {
        eprintln!("SuperEditr: couldn't install the update: {error}");
    }
}

/// Whether to install a release: only one newer than this build.
fn should_install(current: &Version, remote: &Version) -> bool {
    remote > current
}

/// How much of the download has arrived. A server needn't say how big it is.
fn percent_of(received: u64, total: Option<u64>) -> Option<u8> {
    total.filter(|total| *total > 0).map(|total| (received.saturating_mul(100) / total).min(100) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_macos_and_windows_release_builds_update_themselves() {
        assert!(updates_itself(true, true, false), "a macOS release build");
        assert!(updates_itself(true, false, true), "a Windows release build");
        assert!(!updates_itself(true, false, false), "a Linux release build");
        assert!(!updates_itself(false, true, false), "a macOS source build");
        assert!(!updates_itself(false, false, true), "a Windows source build");
    }

    #[test]
    fn a_windows_update_waits_until_nothing_is_going_on() {
        let idle = USER_IDLE;
        assert_eq!(hold(false, false, false, idle), None);
        assert_eq!(hold(false, false, false, idle * 10), None);
        assert_eq!(hold(true, false, false, idle), Some(Hold::Working), "a quick fix running");
        assert_eq!(hold(false, true, false, idle), Some(Hold::Panel), "a review open");
        assert_eq!(hold(false, false, true, idle), Some(Hold::Window), "Settings open");
        assert_eq!(
            hold(false, false, false, idle - Duration::from_secs(1)),
            Some(Hold::UserActive),
            "typing elsewhere"
        );
        assert_eq!(hold(true, true, true, Duration::ZERO), Some(Hold::Working), "the first reason found");
    }

    #[test]
    fn a_poll_long_after_the_last_means_the_computer_slept() {
        assert!(!slept(IDLE_POLL));
        assert!(!slept(IDLE_POLL * 2), "a busy moment");
        assert!(slept(Duration::from_secs(45 * 60)), "a night asleep");
    }

    #[test]
    fn busy_holds_until_dropped() {
        let before = BUSY.load(Ordering::SeqCst);
        let first = busy();
        let second = busy();
        assert_eq!(BUSY.load(Ordering::SeqCst), before + 2);
        drop(first);
        drop(second);
        assert_eq!(BUSY.load(Ordering::SeqCst), before);
    }

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn installs_only_newer_releases() {
        assert!(should_install(&v("1.0.0"), &v("1.0.1")));
        assert!(!should_install(&v("1.0.1"), &v("1.0.1")));
        assert!(!should_install(&v("1.1.0"), &v("1.0.9")));
    }

    #[test]
    fn a_new_check_starts_unless_one_is_running_or_an_update_is_waiting() {
        let version = "1.0.1".to_string();
        for free in [
            Status::Idle,
            Status::UpToDate { version: version.clone() },
            Status::CheckFailed,
            Status::InstallFailed { version: version.clone() },
        ] {
            assert!(free.can_check(), "{free:?}");
        }
        for busy in [
            Status::Checking,
            Status::Downloading { version: version.clone(), percent: None },
            Status::Ready { version },
        ] {
            assert!(!busy.can_check(), "{busy:?}");
        }
    }

    #[test]
    fn the_menu_goes_back_to_its_usual_words_once_a_window_has_the_result() {
        let usual = "Check for Updates…";
        assert_eq!(Status::UpToDate { version: "1.0.0".into() }.menu_line(), usual);
        assert_eq!(Status::CheckFailed.menu_line(), usual);
        assert_eq!(Status::Ready { version: "1.0.1".into() }.menu_line(), "Restart to Update to 1.0.1");
    }

    #[test]
    fn percent_needs_a_known_size_and_stops_at_100() {
        assert_eq!(percent_of(50, Some(200)), Some(25));
        assert_eq!(percent_of(300, Some(200)), Some(100));
        assert_eq!(percent_of(50, None), None);
        assert_eq!(percent_of(50, Some(0)), None);
    }

    #[test]
    fn the_window_reads_the_state_by_name() {
        let json = serde_json::to_value(Status::UpToDate { version: "1.0.0".into() }).unwrap();
        assert_eq!(json, serde_json::json!({ "state": "up-to-date", "version": "1.0.0" }));
    }
}
