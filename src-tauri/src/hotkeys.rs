//! Global keyboard shortcuts.
//!
//! Persistent shortcuts come from settings: review, quick fix, and optional per-preset keys.
//! While the overlay is showing, with or without focus, ⌥↩ applies its suggestion and ⌥⎋
//! dismisses it, so the user never has to reach for the mouse.
//!
//! A shortcut written as the same chord twice ("Command+E Command+E", shown as ⌘E E) is a double
//! press, watched by `platform::start_double_press_tap` instead of the system hotkey API.
//!
//! On Linux, Wayland gives apps no global keys: the compositor binds review and quick fix and
//! hands them to SuperEditr (`platform::set_shortcuts`), and the panel takes focus for its own keys.
//!
//! On Windows, shortcuts are registered with `RegisterHotKey` by `platform::hotkey`, by the virtual
//! key the recorder saved (see `keys`), which the global-shortcut plugin can't do. Windows switches
//! windows with Alt+Esc itself, so there the panel takes plain Esc while it shows, whether or not it
//! has the keyboard, and hands it to the panel's page like a key typed there: an instruction being
//! typed is cancelled first, and otherwise the panel or notice closes. A registered hotkey never
//! reaches a window, so the page sees each press once.

use crate::{double_press, platform, review};
use std::collections::HashMap;
#[cfg(target_os = "macos")]
use std::str::FromStr;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
#[cfg(target_os = "macos")]
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

#[derive(Clone, Debug)]
enum Action {
    Review,
    QuickFix,
    // On Linux, style shortcuts and the panel's own keys aren't bound globally.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Preset(String),
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    OverlayApply,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    OverlayDismiss,
    /// A key for the panel, passed to its page as if typed into it (see `review_keys::Key`).
    #[cfg_attr(not(windows), allow(dead_code))]
    PanelKey(&'static str),
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
#[cfg(not(windows))]
const OVERLAY_DISMISS: &str = "Alt+Escape";
/// Plain Esc, while the panel or a notice shows: Windows uses Alt+Esc to switch windows.
#[cfg(windows)]
const OVERLAY_DISMISS: &str = "Escape";

#[cfg(target_os = "macos")]
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
        Action::PanelKey(key) => {
            let key = crate::review_keys::Key {
                key: key.into(),
                meta_key: false,
                ctrl_key: false,
                alt_key: false,
                shift_key: false,
            };
            let _ = app.emit_to("overlay", "overlay:key", key);
        }
    }
}

/// A shortcut the compositor handed SuperEditr on Linux, run on the main thread like the others.
#[cfg(target_os = "linux")]
pub fn on_linux_shortcut(app: &AppHandle, action: platform::desktop::Action) {
    if *app.state::<Registry>().paused.lock().unwrap() {
        return;
    }
    let action = match action {
        platform::desktop::Action::Review => Action::Review,
        platform::desktop::Action::QuickFix => Action::QuickFix,
    };
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || run(&handle, action));
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
#[cfg(target_os = "linux")]
pub fn taken_by_another_app(_app: &AppHandle, accelerator: &str) -> bool {
    platform::shortcut_taken(accelerator).is_some()
}

#[cfg(windows)]
pub fn taken_by_another_app(app: &AppHandle, accelerator: &str) -> bool {
    let Some(chord) = crate::keys::chord(accelerator) else { return false };
    // One SuperEditr registered itself would test as taken.
    let config = app.state::<crate::state::AppState>().config();
    let ours = [config.hotkeys.review.as_str(), config.hotkeys.quick_fix.as_str(), OVERLAY_APPLY, OVERLAY_DISMISS]
        .into_iter()
        .chain(config.presets.iter().filter_map(|p| p.shortcut.as_deref()))
        .any(|own| crate::keys::chord(own) == Some(chord));
    !ours && matches!(platform::hotkey::available(chord.into()), Err(platform::hotkey::RegisterError::Taken))
}

#[cfg(target_os = "macos")]
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
        #[cfg(target_os = "linux")]
        platform::set_shortcuts(&[]);
        #[cfg(windows)]
        unregister_all(app);
        #[cfg(target_os = "macos")]
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
#[cfg(target_os = "linux")]
pub fn register_from_config(app: &AppHandle) -> Vec<String> {
    use platform::desktop::{Action as DesktopAction, Binding, Shortcut};
    let config = app.state::<crate::state::AppState>().config();
    if *app.state::<Registry>().paused.lock().unwrap() {
        return Vec::new();
    }
    let mut problems = Vec::new();
    let mut bindings = Vec::new();
    let wanted =
        [(&config.hotkeys.review, DesktopAction::Review), (&config.hotkeys.quick_fix, DesktopAction::QuickFix)];
    for (accelerator, action) in wanted {
        if accelerator.trim().is_empty() {
            continue;
        }
        if double_press::parse(accelerator).is_some() {
            problems.push(format!("{accelerator}: double presses work only on macOS for now"));
            continue;
        }
        match Shortcut::parse(accelerator) {
            Some(shortcut) => bindings.push(Binding { action, shortcut }),
            None => problems.push(format!("{accelerator}: not a shortcut SuperEditr can bind")),
        }
    }
    for preset in &config.presets {
        if let Some(key) = preset.shortcut.as_ref().filter(|k| !k.trim().is_empty()) {
            problems.push(format!("{key}: shortcuts for a style work only on macOS for now"));
        }
    }
    problems.extend(platform::set_shortcuts(&bindings));
    problems
}

/// Registers shortcuts from settings. Returns a description of any that failed (usually
/// because another app already owns them).
#[cfg(target_os = "macos")]
pub fn register_from_config(app: &AppHandle) -> Vec<String> {
    let config = app.state::<crate::state::AppState>().config();
    let registry = app.state::<Registry>();
    if *registry.paused.lock().unwrap() {
        return Vec::new();
    }
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();
    *registry.overlay_active.lock().unwrap() = false;
    registry.actions.lock().unwrap().clear();
    // Filled while registering and stored after: see `set_overlay_shortcuts` for why no lock is
    // held across a call to the plugin.
    let mut actions = HashMap::new();

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
    registry.actions.lock().unwrap().extend(actions);
    *registry.double_actions.lock().unwrap() = double_actions;
    platform::set_double_presses(doubles);
    problems
}

/// Adds or removes the overlay-only shortcuts, on the main thread.
///
/// The plugin registers and unregisters a shortcut on the main thread and waits for it, and the
/// shortcut handler runs on the main thread and takes `actions`. A worker that held `actions` (or
/// any lock the main thread may want) while registering waited for the main thread while the main
/// thread waited for it, and every shortcut stopped. So the change runs on the main thread, in
/// order, and no lock is held across a call to the plugin.
#[cfg(target_os = "macos")]
pub fn set_overlay_shortcuts(app: &AppHandle, active: bool) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || set_overlay_shortcuts_now(&handle, active));
}

/// On Linux the panel has focus during a review and takes its keys itself.
#[cfg(target_os = "linux")]
pub fn set_overlay_shortcuts(_app: &AppHandle, _active: bool) {}

#[cfg(target_os = "macos")]
fn set_overlay_shortcuts_now(app: &AppHandle, active: bool) {
    let registry = app.state::<Registry>();
    {
        let mut current = registry.overlay_active.lock().unwrap();
        if *current == active {
            return;
        }
        *current = active;
    }
    // While paused, only the state is kept; `pause` registers them on resuming.
    if *registry.paused.lock().unwrap() {
        return;
    }
    let shortcuts = app.global_shortcut();
    for (accelerator, action) in [(OVERLAY_APPLY, Action::OverlayApply), (OVERLAY_DISMISS, Action::OverlayDismiss)] {
        let Ok(shortcut) = Shortcut::from_str(accelerator) else { continue };
        if active {
            if shortcuts.register(shortcut).is_ok() {
                registry.actions.lock().unwrap().insert(shortcut.id(), action);
            }
        } else {
            let _ = shortcuts.unregister(shortcut);
            registry.actions.lock().unwrap().remove(&shortcut.id());
        }
    }
}

/// Ids SuperEditr's own registrations use with `platform::hotkey`; the panel's keys have fixed ones.
#[cfg(windows)]
const OVERLAY_IDS: [(i32, &str); 2] = [(0xB001, OVERLAY_APPLY), (0xB002, OVERLAY_DISMISS)];

#[cfg(windows)]
fn unregister_all(app: &AppHandle) {
    let registry = app.state::<Registry>();
    let ids: Vec<u32> = registry.actions.lock().unwrap().drain().map(|(id, _)| id).collect();
    for id in ids {
        platform::hotkey::unregister(id as i32);
    }
}

/// A shortcut Windows reported (`WM_HOTKEY`), run on the main thread like the others.
#[cfg(windows)]
pub fn on_windows_hotkey(app: &AppHandle, id: i32) {
    let registry = app.state::<Registry>();
    if *registry.paused.lock().unwrap() {
        return;
    }
    let action = registry.actions.lock().unwrap().get(&(id as u32)).cloned();
    platform::trace(format_args!("shortcut {id:#x}: {action:?}"));
    if let Some(action) = action {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || run(&handle, action));
    }
}

/// Registers shortcuts from settings with `RegisterHotKey`. Returns a description of any that
/// failed.
#[cfg(windows)]
pub fn register_from_config(app: &AppHandle) -> Vec<String> {
    use platform::hotkey::{self, RegisterError};
    let config = app.state::<crate::state::AppState>().config();
    let registry = app.state::<Registry>();
    if *registry.paused.lock().unwrap() {
        return Vec::new();
    }
    unregister_all(app);
    *registry.overlay_active.lock().unwrap() = false;
    let mut wanted =
        vec![(config.hotkeys.review.clone(), Action::Review), (config.hotkeys.quick_fix.clone(), Action::QuickFix)];
    for preset in &config.presets {
        if let Some(key) = preset.shortcut.as_ref().filter(|k| !k.is_empty()) {
            wanted.push((key.clone(), Action::Preset(preset.id.clone())));
        }
    }
    let mut problems = Vec::new();
    let mut actions = HashMap::new();
    for (id, (accelerator, action)) in (1..).zip(wanted) {
        if accelerator.trim().is_empty() {
            continue;
        }
        if double_press::parse(&accelerator).is_some() {
            problems.push(format!("{accelerator}: double presses work only on macOS for now"));
            continue;
        }
        let Some(chord) = crate::keys::chord(&accelerator) else {
            problems.push(format!("{accelerator}: not a shortcut Windows can register"));
            continue;
        };
        match hotkey::register(id, chord.into()) {
            Ok(()) => {
                actions.insert(id as u32, action);
            }
            Err(RegisterError::Taken) => problems.push(format!("{accelerator}: Windows or another app uses this")),
            Err(RegisterError::Failed(why)) => problems.push(format!("{accelerator}: {why}")),
        }
    }
    registry.actions.lock().unwrap().extend(actions);
    platform::set_double_presses(Vec::new());
    problems
}

/// The panel's own keys while it shows: Alt+Enter applies, and Esc goes to the panel's page.
#[cfg(windows)]
fn overlay_actions() -> [(i32, &'static str, Action); 2] {
    let [(apply_id, apply), (close_id, close)] = OVERLAY_IDS;
    [(apply_id, apply, Action::OverlayApply), (close_id, close, Action::PanelKey("Escape"))]
}

/// Adds or removes the panel's Alt+Enter and Esc, while a panel or notice shows. Esc that can't be
/// registered (another app has it) is logged, and the panel then takes it only with the keyboard.
#[cfg(windows)]
pub fn set_overlay_shortcuts(app: &AppHandle, active: bool) {
    let registry = app.state::<Registry>();
    {
        let mut current = registry.overlay_active.lock().unwrap();
        if *current == active {
            return;
        }
        *current = active;
    }
    if *registry.paused.lock().unwrap() {
        return;
    }
    for (id, accelerator, action) in overlay_actions() {
        if active {
            let registered = crate::keys::chord(accelerator).map(|chord| platform::hotkey::register(id, chord.into()));
            match registered {
                Some(Ok(())) => {
                    registry.actions.lock().unwrap().insert(id as u32, action);
                }
                other => platform::trace(format_args!("{accelerator}: not registered ({other:?})")),
            }
        } else {
            platform::hotkey::unregister(id);
            registry.actions.lock().unwrap().remove(&(id as u32));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panels_keys_are_listed_for_the_shortcut_check() {
        let others = others(&supereditr_core::Config::default(), "review");
        assert!(others.iter().any(|(key, what)| key == OVERLAY_APPLY && what == "applying a suggestion"));
        assert!(others.iter().any(|(key, what)| key == OVERLAY_DISMISS && what == "closing a suggestion"));
    }

    #[cfg(windows)]
    #[test]
    fn plain_esc_goes_to_the_panel_as_a_key() {
        let [(_, apply, apply_action), (_, close, close_action)] = overlay_actions();
        assert_eq!(crate::keys::chord(apply), Some(crate::keys::Chord { modifiers: double_press::ALT, vk: 0x0D }));
        assert!(matches!(apply_action, Action::OverlayApply));
        assert_eq!(crate::keys::chord(close), Some(crate::keys::Chord { modifiers: 0, vk: 0x1B }), "no modifiers");
        assert!(matches!(close_action, Action::PanelKey("Escape")), "through the page, so / typing is cancelled first");
    }
}
