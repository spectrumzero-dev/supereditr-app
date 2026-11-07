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

use semver::Version;
use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::MenuItem;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_updater::UpdaterExt;

pub const ENABLED: bool = option_env!("SUPEREDITR_RELEASE_BUILD").is_some();

/// How long after launch the first automatic check waits, and how often it repeats.
const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// Without limits a stalled request would never finish, and the check would never end.
const FEED_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);

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

/// The feed and key the UI smoke tests give a test build. Official builds ignore them.
fn test_feed() -> Option<(String, String)> {
    if ENABLED {
        return None;
    }
    Some((std::env::var("SUPEREDITR_TEST_UPDATE_FEED").ok()?, std::env::var("SUPEREDITR_TEST_UPDATE_KEY").ok()?))
}

/// Whether this copy of the app can update itself.
pub fn available() -> bool {
    ENABLED || test_feed().is_some()
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
    app.manage(UpdateItem { item: item.clone(), status: Mutex::new(status) });
    Ok(item)
}

/// Checks soon after launch and then every few hours, without asking.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
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
        app.restart();
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
    let updater = app
        .updater_builder()
        .timeout(FEED_TIMEOUT)
        .version_comparator(|current, remote| should_install(&current, &remote.version))
        .build();
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
            match update.download_and_install(progress, || {}).await {
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
    state.set(app, result);
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
