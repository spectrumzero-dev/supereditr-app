//! SuperEditr desktop shell: menu-bar app, global shortcuts, overlay, and settings.

mod commands;
mod double_press;
mod hotkeys;
mod overlay;
mod permission_guide;
mod platform;
mod review;
mod review_keys;
mod secrets;
mod shortcut_check;
mod state;
mod updater;

use state::AppState;
use std::sync::atomic::Ordering;
use supereditr_core::install::Install;
use supereditr_core::provider::{ProviderKind, ProviderProfile};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

/// Keeps `status.json` (next to settings.json) current with whether macOS trusts SuperEditr for
/// Accessibility. Only the app itself can ask, so this is how scripts and UI tests check it.
fn write_status_file(path: std::path::PathBuf) {
    std::thread::spawn(move || {
        let mut last = None;
        loop {
            let trusted = platform::accessibility_trusted(false);
            if last != Some(trusted) {
                let status = serde_json::json!({
                    "accessibility": trusted,
                    "version": env!("CARGO_PKG_VERSION"),
                    "pid": std::process::id(),
                    "updated": std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                });
                let tmp = path.with_extension("json.tmp");
                if std::fs::write(&tmp, status.to_string()).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
                    last = Some(trusted);
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}

/// SuperEditr is a menu-bar app, out of the Dock and ⌘Tab, except while Settings is open: then it
/// behaves like a normal app so the user can switch back to it.
fn set_in_dock(app: &AppHandle, in_dock: bool) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(if in_dock {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    });
    #[cfg(not(target_os = "macos"))]
    let _ = (app, in_dock);
}

pub fn show_settings(app: &AppHandle, section: Option<String>) {
    if let Some(window) = app.get_webview_window("settings") {
        set_in_dock(app, true);
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        if let Some(section) = section {
            let _ = app.emit_to("settings", "settings:section", section);
        }
    }
}

/// The menu-bar menu's first line: which AI reviews, or what's wrong with it.
struct AiStatusItem(MenuItem<tauri::Wry>);

/// Checks the AI in use and shows the result in the menu-bar menu.
pub fn show_ai_status(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let label = match state.active_profile() {
            Some(profile) => {
                let name = profile.kind.display_name();
                let result = commands::check(&state, &profile.id).await;
                match result.problem() {
                    None => format!("AI: {name}"),
                    Some(problem) => format!("AI: {name} isn’t working ({problem})"),
                }
            }
            None => match state.engine.builtin().await {
                Some(_) => "AI: Apple Intelligence (built in)".into(),
                None => "AI: none (rule-based spelling and grammar)".into(),
            },
        };
        if let Some(item) = app.try_state::<AiStatusItem>() {
            let _ = item.0.set_text(label);
        }
    });
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "ai-status", "AI: checking…", false, None::<&str>)?;
    app.manage(AiStatusItem(status.clone()));
    let review = MenuItem::with_id(app, "review", "Review Selection", true, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, "pause", "Pause Automatic Suggestions", true, false, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?;
    let quit = MenuItem::with_id(app, "quit", "Quit SuperEditr", true, Some("CmdOrCtrl+Q"))?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &review,
            &pause,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    if updater::ENABLED {
        // Below Settings…
        menu.insert(&updater::menu_item(app)?, 6)?;
    }
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
    let pause_item = pause.clone();
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("SuperEditr")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "review" => review::on_review_hotkey(app),
            "pause" => {
                let paused = pause_item.is_checked().unwrap_or(false);
                app.state::<AppState>().paused.store(paused, Ordering::Relaxed);
            }
            "settings" => show_settings(app, None),
            "update" => updater::on_menu(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

/// Opening at login is on by default: turned on once, before Settings first reads the config, and
/// then left to the user. A development build outside an app bundle is never added.
fn launch_at_login_by_default(app: &AppHandle) {
    use tauri_plugin_autostart::ManagerExt;
    let state = app.state::<AppState>();
    let mut config = state.config();
    let bundled = std::env::current_exe().is_ok_and(|exe| exe.to_string_lossy().contains(".app/Contents/MacOS/"));
    if config.offered_launch_at_login || !bundled {
        return;
    }
    if let Err(e) = app.autolaunch().enable() {
        eprintln!("SuperEditr: couldn't turn on opening at login: {e}");
        return;
    }
    config.offered_launch_at_login = true;
    if let Err(e) = state.save_config(config) {
        eprintln!("SuperEditr: couldn't save settings: {e}");
    }
}

pub fn run() {
    let mut builder = tauri::Builder::default();
    if updater::ENABLED {
        builder = builder.plugin(updater::plugin());
    }
    builder
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_settings(app, None)))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .plugin(hotkeys::plugin())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                app.handle().plugin(tauri_nspanel::init())?;
            }
            let config_dir = app.path().app_config_dir()?;
            let settings = config_dir.join("settings.json");
            let install = config_dir.join("install.json");
            if let Err(e) = Install::record(&install, env!("CARGO_PKG_VERSION"), settings.exists()) {
                eprintln!("SuperEditr: couldn't record the first run in {}: {e}", install.display());
            }
            app.manage(AppState::new(settings));
            launch_at_login_by_default(app.handle());
            app.manage(hotkeys::Registry::default());
            write_status_file(config_dir.join("status.json"));

            let handle = app.handle().clone();
            overlay::init(&handle);
            permission_guide::init(&handle);
            if std::env::var_os("SUPEREDITR_PREVIEW_GUIDE").is_some() {
                permission_guide::start(&handle);
            }
            build_tray(&handle)?;
            show_ai_status(&handle);
            if updater::ENABLED {
                updater::start(&handle);
            }
            let problems = hotkeys::register_from_config(&handle);
            hotkeys::start_double_presses(&handle);
            if !problems.is_empty() {
                eprintln!("SuperEditr: some shortcuts could not be registered: {problems:?}");
            }

            // Load the offline checker's dictionary and open a connection to the active provider
            // now, so the first review is as fast as the rest.
            let state = handle.state::<AppState>();
            let engine = state.engine.clone();
            let profile = state.active_profile();
            let words = state.config().words;
            tauri::async_runtime::spawn(async move {
                let warm = engine.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || warm.prepare(&words)).await;
                // With no other AI chosen, Apple Intelligence reviews; warming it is a no-op where
                // it's unavailable.
                let profile = profile.unwrap_or_else(|| ProviderProfile::new(ProviderKind::AppleIntelligence));
                engine.providers.warm(&profile).await;
            });

            #[cfg(target_os = "macos")]
            {
                platform::refresh_keyboard_layout();
                // The selection watcher's system-wide key monitor counts, to macOS, as asking for
                // Accessibility: installed without permission, it lists SuperEditr there (switched
                // off) at launch. So it starts only once access is granted.
                let watcher_app = handle.clone();
                std::thread::spawn(move || {
                    while !platform::accessibility_trusted(false) {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                    let app = watcher_app.clone();
                    let _ = watcher_app.run_on_main_thread(move || {
                        platform::start_selection_watcher(move |event| review::on_watch_event(&app, event));
                    });
                });
                let switch_app = handle.clone();
                platform::start_app_switch_watcher(move |pid| review::on_app_switch(&switch_app, pid));
            }

            if let Some(settings) = handle.get_webview_window("settings") {
                let window = settings.clone();
                let app = handle.clone();
                settings.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = window.hide();
                        set_in_dock(&app, false);
                    }
                });
            }
            let first_run = !handle.state::<AppState>().config().onboarded;
            if first_run || !platform::accessibility_trusted(false) {
                show_settings(&handle, Some(if first_run { "welcome".into() } else { "permissions".into() }));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::run_preset,
            commands::apply_text,
            commands::dismiss,
            commands::copy_text,
            commands::resize_overlay,
            commands::focus_overlay,
            commands::overlay_typing,
            commands::open_settings,
            commands::add_word,
            commands::get_settings,
            commands::save_settings,
            commands::set_api_key,
            commands::scan_accounts,
            commands::adopt_candidate,
            commands::add_provider,
            commands::remove_provider,
            commands::list_models,
            commands::test_provider,
            commands::pull_ollama_model,
            commands::builtin_ai,
            commands::default_presets,
            commands::check_shortcut,
            commands::check_provider,
            commands::activate_provider,
            commands::review_playground,
            commands::fit_settings_height,
            commands::pause_shortcuts,
            commands::open_apple_intelligence_settings,
            commands::launch_at_login,
            commands::set_launch_at_login,
            commands::accessibility_status,
            commands::open_accessibility_settings,
            commands::guide_accessibility,
            commands::close_guide,
            commands::drag_app,
            commands::preview_review,
            commands::set_paused,
        ])
        .run(tauri::generate_context!())
        .expect("SuperEditr failed to start");
}
