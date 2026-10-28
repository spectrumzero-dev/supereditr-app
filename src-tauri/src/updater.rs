//! Updates for official release builds. The app reads `latest.json` from the public repository's
//! newest GitHub Release, checks the download against SuperEditr's own updater key (the public half
//! is in tauri.conf.json), and installs it in place; the new version runs after a restart.
//!
//! Only official release builds check for updates: they are compiled with
//! `SUPEREDITR_RELEASE_BUILD` set. Source builds and test builds never replace themselves, because an
//! official build is signed differently and macOS would ask for Accessibility permission again.

use semver::Version;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::menu::MenuItem;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::UpdaterExt;

pub const ENABLED: bool = option_env!("SUPEREDITR_RELEASE_BUILD").is_some();

/// How long after launch the first automatic check waits, and how often it repeats.
const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// Without limits a stalled request would never finish, and the menu item would stay disabled.
const FEED_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The menu-bar menu's update line, and whether an installed update is waiting for a restart.
pub struct UpdateItem {
    item: MenuItem<tauri::Wry>,
    ready: AtomicBool,
    checking: AtomicBool,
}

pub fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry, tauri_plugin_updater::Config> {
    tauri_plugin_updater::Builder::new().build()
}

pub fn menu_item(app: &AppHandle) -> tauri::Result<MenuItem<tauri::Wry>> {
    let item = MenuItem::with_id(app, "update", "Check for Updates…", true, None::<&str>)?;
    app.manage(UpdateItem { item: item.clone(), ready: AtomicBool::new(false), checking: AtomicBool::new(false) });
    Ok(item)
}

/// Checks soon after launch and then every few hours, without asking.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            check(&app, false).await;
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// The menu item: restart into an installed update, or check now.
pub fn on_menu(app: &AppHandle) {
    let Some(state) = app.try_state::<UpdateItem>() else { return };
    if state.ready.load(Ordering::Relaxed) {
        app.restart();
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move { check(&app, true).await });
}

async fn check(app: &AppHandle, manual: bool) {
    let Some(state) = app.try_state::<UpdateItem>() else { return };
    if state.ready.load(Ordering::Relaxed) || state.checking.swap(true, Ordering::Relaxed) {
        return;
    }
    let show = |text: &str, enabled: bool| {
        let _ = state.item.set_text(text);
        let _ = state.item.set_enabled(enabled);
    };
    if manual {
        show("Checking for Updates…", false);
    }
    let updater = app
        .updater_builder()
        .timeout(FEED_TIMEOUT)
        .version_comparator(|current, remote| should_install(&current, &remote.version))
        .build();
    let found = match updater {
        Ok(updater) => updater.check().await,
        Err(error) => Err(error),
    };
    match found {
        Ok(Some(mut update)) => {
            // The feed's timeout doesn't carry over to the download.
            update.timeout = Some(DOWNLOAD_TIMEOUT);
            show(&format!("Downloading SuperEditr {}…", update.version), false);
            match update.download_and_install(|_, _| {}, || {}).await {
                Ok(()) => {
                    state.ready.store(true, Ordering::Relaxed);
                    show(&format!("Restart to Update to {}", update.version), true);
                }
                Err(error) => {
                    eprintln!("SuperEditr: couldn't install update {}: {error}", update.version);
                    show("Update Failed. Try Again…", true);
                }
            }
        }
        Ok(None) => show(if manual { "SuperEditr Is Up to Date" } else { "Check for Updates…" }, true),
        Err(error) => {
            eprintln!("SuperEditr: couldn't check for updates: {error}");
            show(if manual { "Couldn’t Check for Updates. Try Again…" } else { "Check for Updates…" }, true);
        }
    }
    state.checking.store(false, Ordering::Relaxed);
}

/// Whether to install a release: only one newer than this build.
fn should_install(current: &Version, remote: &Version) -> bool {
    remote > current
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
}
