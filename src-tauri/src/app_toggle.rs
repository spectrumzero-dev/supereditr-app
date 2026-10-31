//! The menu-bar menu's "Turn off in <app>" item, which adds the app the user was in to
//! `excluded_apps`, or takes it off ("Turn on in <app>").
//!
//! The label has to be right before the menu opens, which nothing announces, so the item follows
//! the app-switch watcher. SuperEditr's own windows are skipped: with Settings in front, the item is
//! for the app the user came from.

use crate::platform;
use crate::state::AppState;
use std::sync::Mutex;
use supereditr_core::Config;
use supereditr_core::config::is_terminal;
use tauri::menu::{Menu, MenuItem};
use tauri::{AppHandle, Emitter, Manager, Wry};

pub const ID: &str = "app-toggle";

/// The app the item acts on.
#[derive(Debug, Clone, PartialEq)]
struct TargetApp {
    bundle_id: String,
    name: String,
}

/// Whether the menu offers to turn SuperEditr off or on in this app. An app without a bundle ID
/// can't be listed, and an excluded terminal stays off: there, pasting a revision would run it as
/// input. Settings → Privacy can still turn a terminal on.
pub fn offered(config: &Config, bundle_id: &str) -> bool {
    !bundle_id.is_empty() && !(is_terminal(bundle_id) && config.is_excluded(bundle_id))
}

/// What the item says for `app`, or None when it's hidden.
fn label(config: &Config, app: &TargetApp) -> Option<String> {
    if !offered(config, &app.bundle_id) {
        return None;
    }
    // The menu takes a single `&` as a mnemonic marker and drops it.
    let name = app.name.replace('&', "&&");
    Some(if config.is_excluded(&app.bundle_id) { format!("Turn on in {name}") } else { format!("Turn off in {name}") })
}

struct AppToggle {
    menu: Menu<Wry>,
    item: MenuItem<Wry>,
    /// Where the item goes back in when it has been hidden.
    position: usize,
    target: Mutex<Option<TargetApp>>,
}

fn target_for(pid: i32) -> Option<TargetApp> {
    if pid == platform::own_pid() {
        return None;
    }
    let (bundle_id, name) = platform::app_identity(pid);
    Some(TargetApp { bundle_id, name })
}

/// Starts following the app in front. `item` must already be in `menu`.
pub fn init(app: &AppHandle, menu: &Menu<Wry>, item: MenuItem<Wry>) -> tauri::Result<()> {
    let position = menu.items()?.iter().position(|i| i.id() == ID).unwrap_or(0);
    let target = platform::frontmost_pid().and_then(target_for);
    app.manage(AppToggle { menu: menu.clone(), item, position, target: Mutex::new(target) });
    refresh(app);
    Ok(())
}

/// Another app came to the front.
pub fn on_app_switch(app: &AppHandle, pid: i32) {
    let Some(target) = target_for(pid) else { return };
    let Some(toggle) = app.try_state::<AppToggle>() else { return };
    *toggle.target.lock().unwrap() = Some(target);
    refresh(app);
}

/// Shows the item as the current settings have it, or hides it.
pub fn refresh(app: &AppHandle) {
    let Some(toggle) = app.try_state::<AppToggle>() else { return };
    let config = app.state::<AppState>().config();
    let label = toggle.target.lock().unwrap().as_ref().and_then(|target| label(&config, target));
    let shown = toggle.menu.get(ID).is_some();
    match label {
        Some(label) => {
            let _ = toggle.item.set_text(label);
            if !shown {
                let _ = toggle.menu.insert(&toggle.item, toggle.position);
            }
        }
        None if shown => {
            let _ = toggle.menu.remove(&toggle.item);
        }
        None => {}
    }
}

/// What to say when saving the change failed, so SuperEditr is still on, or still off, in the app.
fn failure_message(app: &TargetApp, excluding: bool, error: &str) -> String {
    let (turn, still) = if excluding { ("off", "on") } else { ("on", "off") };
    format!("Couldn’t turn SuperEditr {turn} in {}, so it’s still {still} there: {error}", app.name)
}

/// The item was chosen: saves the change as Settings does, and shows it there.
pub fn on_menu(app: &AppHandle) {
    let Some(toggle) = app.try_state::<AppToggle>() else { return };
    let Some(target) = toggle.target.lock().unwrap().clone() else { return };
    let state = app.state::<AppState>();
    let mut config = state.config();
    if !offered(&config, &target.bundle_id) {
        return;
    }
    let excluding = !config.is_excluded(&target.bundle_id);
    config.set_excluded(&target.bundle_id, excluding);
    if let Err(e) = crate::commands::save_settings(app.clone(), state, config, None) {
        eprintln!("SuperEditr: couldn't save settings: {e}");
        return crate::review::show_error(app, failure_message(&target, excluding, &e));
    }
    let _ = app.emit_to("settings", "settings:reload", ());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(bundle_id: &str, name: &str) -> TargetApp {
        TargetApp { bundle_id: bundle_id.into(), name: name.into() }
    }

    #[test]
    fn offers_to_turn_off_an_app_that_is_on() {
        let config = Config::default();
        assert_eq!(label(&config, &app("com.apple.TextEdit", "TextEdit")).as_deref(), Some("Turn off in TextEdit"));
    }

    #[test]
    fn offers_to_turn_on_an_excluded_app() {
        let mut config = Config::default();
        config.set_excluded("com.apple.TextEdit", true);
        assert_eq!(label(&config, &app("com.apple.TextEdit", "TextEdit")).as_deref(), Some("Turn on in TextEdit"));
        let password_manager = app("com.1password.1password", "1Password");
        assert_eq!(label(&config, &password_manager).as_deref(), Some("Turn on in 1Password"));
    }

    #[test]
    fn choosing_the_item_twice_puts_the_app_back() {
        let mut config = Config::default();
        let before = config.excluded_apps.clone();
        let target = app("com.apple.Notes", "Notes");
        for expected in ["Turn on in Notes", "Turn off in Notes"] {
            let excluded = config.is_excluded(&target.bundle_id);
            assert!(config.set_excluded(&target.bundle_id, !excluded));
            assert_eq!(label(&config, &target).as_deref(), Some(expected));
        }
        assert_eq!(config.excluded_apps, before);
    }

    #[test]
    fn hides_for_an_excluded_terminal() {
        let mut config = Config::default();
        let terminal = app("com.apple.Terminal", "Terminal");
        assert_eq!(label(&config, &terminal), None);
        assert!(!offered(&config, "com.mitchellh.ghostty"));
        // Once the user has turned a terminal on in Settings, the menu can turn it off again.
        config.set_excluded("com.apple.Terminal", false);
        assert_eq!(label(&config, &terminal).as_deref(), Some("Turn off in Terminal"));
    }

    #[test]
    fn hides_for_an_app_without_a_bundle_id() {
        assert_eq!(label(&Config::default(), &app("", "this app")), None);
    }

    #[test]
    fn keeps_an_ampersand_in_the_name() {
        let label = label(&Config::default(), &app("com.example.quillpoint", "Quill & Point"));
        assert_eq!(label.as_deref(), Some("Turn off in Quill && Point"));
    }

    #[test]
    fn a_failed_save_says_supereditr_is_unchanged() {
        let notes = app("com.apple.Notes", "Notes");
        assert_eq!(
            failure_message(&notes, true, "Permission denied"),
            "Couldn’t turn SuperEditr off in Notes, so it’s still on there: Permission denied"
        );
        assert_eq!(
            failure_message(&notes, false, "Permission denied"),
            "Couldn’t turn SuperEditr on in Notes, so it’s still off there: Permission denied"
        );
    }

    #[test]
    fn never_targets_supereditr_itself() {
        assert_eq!(target_for(platform::own_pid()), None);
    }
}
