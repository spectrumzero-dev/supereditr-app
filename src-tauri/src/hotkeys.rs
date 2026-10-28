//! Global keyboard shortcuts.
//!
//! Persistent shortcuts come from settings: review, quick fix, and optional per-preset keys.
//! While the overlay is showing, with or without focus, ⌥↩ applies its suggestion and ⌥⎋
//! dismisses it, so the user never has to reach for the mouse.
//!
//! A shortcut written as the same chord twice ("Command+E Command+E", shown as ⌘E E) is a double
//! press, watched by `platform::start_double_press_tap` instead of the system hotkey API.

use crate::{double_press, platform, review};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

#[derive(Clone, Debug)]
enum Action {
    Review,
    QuickFix,
    Preset(String),
    OverlayApply,
    OverlayDismiss,
}

#[derive(Default)]
pub struct Registry {
    actions: Mutex<HashMap<u32, Action>>,
    /// Actions for double-press shortcuts, in the order given to the tap.
    double_actions: Mutex<Vec<Action>>,
    overlay_active: Mutex<bool>,
    /// Settings is recording a shortcut: every SuperEditr shortcut is off, so the keys reach it.
    paused: Mutex<bool>,
}

const OVERLAY_APPLY: &str = "Alt+Enter";
const OVERLAY_DISMISS: &str = "Alt+Escape";

pub fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            let action = app.state::<Registry>().actions.lock().unwrap().get(&shortcut.id()).cloned();
            if let Some(action) = action {
                run(app, action);
            }
        })
        .build()
}

fn run(app: &AppHandle, action: Action) {
    match action {
        Action::Review => review::on_review_hotkey(app),
        Action::QuickFix => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { review::quick_fix(&app).await });
        }
        Action::Preset(id) => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { review::start_manual(&app, Some(id)).await });
        }
        Action::OverlayApply => {
            let _ = app.emit_to("overlay", "overlay:command", "apply");
        }
        Action::OverlayDismiss => review::dismiss(app, None),
    }
}

/// Starts the double-press watcher, which also passes the review panel its keys (see
/// `review_keys`). Its callbacks run on the tap's thread, so actions are sent to the main thread
/// like the system hotkey handler's.
pub fn start_double_presses(app: &AppHandle) {
    let keys = app.clone();
    let app = app.clone();
    platform::start_double_press_tap(
        move |index| {
            let action = app.state::<Registry>().double_actions.lock().unwrap().get(index).cloned();
            if let Some(action) = action {
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || run(&handle, action));
            }
        },
        move |key| {
            let _ = keys.emit_to("overlay", "overlay:key", key);
        },
    );
}

/// SuperEditr's shortcuts other than `slot` ("review", "quick_fix", or "preset:<id>"), with what
/// each does, for `shortcut_check::conflict`.
pub fn others(config: &supereditr_core::Config, slot: &str) -> Vec<(String, String)> {
    let mut all = vec![
        ("review".to_string(), config.hotkeys.review.clone(), "Review selection".to_string()),
        ("quick_fix".to_string(), config.hotkeys.quick_fix.clone(), "Quick fix".to_string()),
        (String::new(), OVERLAY_APPLY.to_string(), "applying a suggestion".to_string()),
        (String::new(), OVERLAY_DISMISS.to_string(), "closing a suggestion".to_string()),
    ];
    for p in &config.presets {
        if let Some(key) = &p.shortcut {
            all.push((format!("preset:{}", p.id), key.clone(), format!("the “{}” style", p.name)));
        }
    }
    all.into_iter()
        .filter(|(s, key, _)| s != slot && !key.trim().is_empty())
        .map(|(_, key, what)| (key, what))
        .collect()
}

/// Whether another app already holds this key as a global hotkey: macOS refuses to register it
/// for SuperEditr too. The test registration is removed at once. Skipped for a key SuperEditr
/// itself has registered.
pub fn taken_by_another_app(app: &AppHandle, accelerator: &str) -> bool {
    let chord = accelerator.split_whitespace().next().unwrap_or_default();
    let Ok(shortcut) = Shortcut::from_str(chord) else { return false };
    let shortcuts = app.global_shortcut();
    if shortcuts.is_registered(shortcut) {
        return false;
    }
    match shortcuts.register(shortcut) {
        Ok(()) => {
            let _ = shortcuts.unregister(shortcut);
            false
        }
        Err(_) => true,
    }
}

/// Turns every SuperEditr shortcut off while Settings records a new one, so a key already assigned
/// (Quick fix, a style) reaches the recorder instead of running, then turns them back on.
pub fn pause(app: &AppHandle, paused: bool) {
    let registry = app.state::<Registry>();
    {
        let mut current = registry.paused.lock().unwrap();
        if *current == paused {
            return;
        }
        *current = paused;
    }
    platform::pause_double_presses(paused);
    if paused {
        let _ = app.global_shortcut().unregister_all();
        registry.actions.lock().unwrap().clear();
    } else {
        let overlay = *registry.overlay_active.lock().unwrap();
        register_from_config(app);
        if overlay {
            set_overlay_shortcuts(app, true);
        }
    }
}

/// Registers shortcuts from settings. Returns a description of any that failed (usually
/// because another app already owns them).
pub fn register_from_config(app: &AppHandle) -> Vec<String> {
    let config = app.state::<crate::state::AppState>().config();
    let registry = app.state::<Registry>();
    if *registry.paused.lock().unwrap() {
        return Vec::new();
    }
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();
    *registry.overlay_active.lock().unwrap() = false;
    let mut actions = registry.actions.lock().unwrap();
    actions.clear();

    let mut wanted =
        vec![(config.hotkeys.review.clone(), Action::Review), (config.hotkeys.quick_fix.clone(), Action::QuickFix)];
    for preset in &config.presets {
        if let Some(key) = preset.shortcut.as_ref().filter(|k| !k.is_empty()) {
            wanted.push((key.clone(), Action::Preset(preset.id.clone())));
        }
    }
    let mut problems = Vec::new();
    let mut doubles = Vec::new();
    let mut double_actions = Vec::new();
    for (accelerator, action) in wanted {
        if accelerator.trim().is_empty() {
            continue;
        }
        match double_press::parse(&accelerator) {
            Some(Ok(chord)) if cfg!(target_os = "macos") => {
                doubles.push(chord);
                double_actions.push(action);
                continue;
            }
            Some(Ok(_)) => {
                problems.push(format!("{accelerator}: double presses work only on macOS for now"));
                continue;
            }
            Some(Err(e)) => {
                problems.push(format!("{accelerator}: {e}"));
                continue;
            }
            None => {}
        }
        match Shortcut::from_str(&accelerator) {
            Ok(shortcut) => match shortcuts.register(shortcut) {
                Ok(()) => {
                    actions.insert(shortcut.id(), action);
                }
                Err(e) => problems.push(format!("{accelerator}: {e}")),
            },
            Err(e) => problems.push(format!("{accelerator}: {e}")),
        }
    }
    *registry.double_actions.lock().unwrap() = double_actions;
    platform::set_double_presses(doubles);
    problems
}

/// Adds or removes the overlay-only shortcuts.
pub fn set_overlay_shortcuts(app: &AppHandle, active: bool) {
    let registry = app.state::<Registry>();
    let mut current = registry.overlay_active.lock().unwrap();
    if *current == active {
        return;
    }
    *current = active;
    // While paused, only the state is kept; `pause` registers them on resuming.
    if *registry.paused.lock().unwrap() {
        return;
    }
    let shortcuts = app.global_shortcut();
    let mut actions = registry.actions.lock().unwrap();
    for (accelerator, action) in [(OVERLAY_APPLY, Action::OverlayApply), (OVERLAY_DISMISS, Action::OverlayDismiss)] {
        let Ok(shortcut) = Shortcut::from_str(accelerator) else { continue };
        if active {
            if shortcuts.register(shortcut).is_ok() {
                actions.insert(shortcut.id(), action);
            }
        } else {
            let _ = shortcuts.unregister(shortcut);
            actions.remove(&shortcut.id());
        }
    }
}
