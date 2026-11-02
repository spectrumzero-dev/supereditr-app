//! SuperEditr desktop shell: menu-bar app, global shortcuts, overlay, and settings.

mod app_toggle;
mod apps;
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
#[cfg(target_os = "linux")]
mod tray_linux;
mod updater;

use state::AppState;
#[cfg(not(target_os = "linux"))]
use std::sync::atomic::Ordering;
use supereditr_core::install::Install;
use supereditr_core::provider::{ProviderKind, ProviderProfile};
#[cfg(not(target_os = "linux"))]
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
#[cfg(not(target_os = "linux"))]
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
#[cfg(not(target_os = "linux"))]
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
        #[cfg(target_os = "linux")]
        tray_linux::refresh(&app, Some(label));
        #[cfg(not(target_os = "linux"))]
        if let Some(item) = app.try_state::<AiStatusItem>() {
            let _ = item.0.set_text(label);
        }
    });
}

/// The menu-bar glyph is black, and macOS tints it as a template. Linux trays draw it as it is, and
/// Omarchy's bar is dark, so Linux gets a light one.
#[cfg(not(target_os = "linux"))]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray.png");
#[cfg(target_os = "linux")]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray-light.png");

/// On Linux the tray is `tray_linux`'s, which answers a left click; its menu has no key equivalents
/// and no "Pause", since automatic suggestions are macOS-only for now.
#[cfg(target_os = "linux")]
fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    app_toggle::init_without_menu(app);
    tray_linux::start(app, TRAY_ICON);
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "ai-status", "AI: checking…", false, None::<&str>)?;
    app.manage(AiStatusItem(status.clone()));
    let review = MenuItem::with_id(app, "review", "Review Selection", true, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, "pause", "Pause Automatic Suggestions", true, false, None::<&str>)?;
    // Labelled and shown or hidden by `app_toggle`.
    let app_toggle = MenuItem::with_id(app, app_toggle::ID, "", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?;
    let quit = MenuItem::with_id(app, "quit", "Quit SuperEditr", true, Some("CmdOrCtrl+Q"))?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &review,
            &pause,
            &app_toggle,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    if updater::ENABLED {
        // Below Settings…
        menu.insert(&updater::menu_item(app)?, 7)?;
    }
    app_toggle::init(app, &menu, app_toggle)?;
    let icon = tauri::image::Image::from_bytes(TRAY_ICON)?;
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
            app_toggle::ID => app_toggle::on_menu(app),
            "settings" => show_settings(app, None),
            "update" => updater::on_menu(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

/// Whether a Launch Agent pointing at this executable keeps working after a restart: it's inside an
/// app bundle, and not on a mounted disk image or in the temporary copy macOS runs a downloaded app
/// from until it's moved (App Translocation).
#[cfg(target_os = "macos")]
fn lasting_app_path(exe: &str) -> bool {
    exe.contains(".app/Contents/MacOS/") && !exe.starts_with("/Volumes/") && !exe.contains("/AppTranslocation/")
}

/// Whether an autostart entry pointing at this executable keeps working after a restart: it's
/// installed, not in a temporary folder (where an AppImage is mounted while it runs) or a build
/// folder. The entry's `Exec=` line holds the path unquoted, and systemd skips an entry it can't
/// parse without saying so, so a path with characters that need quoting doesn't count either.
#[cfg(not(target_os = "macos"))]
fn lasting_app_path(exe: &str) -> bool {
    let plain = exe.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c));
    let temporary = ["/tmp/", "/var/tmp/", "/run/", "/dev/shm/"].iter().any(|dir| exe.starts_with(dir));
    let build = exe.contains("/target/debug/") || exe.contains("/target/release/");
    exe.starts_with('/') && plain && !temporary && !build
}

/// The file that opens SuperEditr at login, as the autostart plugin writes it.
fn login_item(app: &AppHandle) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    return app.path().home_dir().ok().map(|home| home.join("Library/LaunchAgents/SuperEditr.plist"));
    #[cfg(not(target_os = "macos"))]
    return app.path().config_dir().ok().map(|config| config.join(format!("autostart/{AUTOSTART_NAME}.desktop")));
}

/// The autostart entry's name on Linux: `~/.config/autostart/supereditr.desktop`.
#[cfg(not(target_os = "macos"))]
const AUTOSTART_NAME: &str = "supereditr";

/// The app's identifier, which names its settings folder.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const IDENTIFIER: &str = "app.supereditr";

/// The app's identifier before 0.1.2. It named the settings folder and the Keychain service.
const OLD_IDENTIFIER: &str = "com.sghael.supereditr";

/// Copies the settings and first-run record from the folder named after the old identifier, the
/// first time this version runs, so an update isn't taken for a first run. The old folder stays as
/// it is. Returns whether it copied the settings.
fn copy_old_settings(config_dir: &std::path::Path) -> std::io::Result<bool> {
    let old = config_dir.with_file_name(OLD_IDENTIFIER);
    if config_dir.join("settings.json").exists() || !old.join("settings.json").is_file() {
        return Ok(false);
    }
    std::fs::create_dir_all(config_dir)?;
    let copy = |name: &str| -> std::io::Result<()> {
        let from = old.join(name);
        if !from.is_file() {
            return Ok(());
        }
        let to = config_dir.join(name);
        let tmp = to.with_extension("json.tmp");
        std::fs::copy(from, &tmp)?;
        std::fs::rename(tmp, to)
    };
    // A first-run record that can't be copied doesn't stop the settings: `Install::record` then
    // writes a new one. settings.json goes last, and appears whole: once it's there, the copy is
    // done.
    if let Err(e) = copy("install.json") {
        eprintln!("SuperEditr: couldn't copy the first-run record from before 0.1.2: {e}");
    }
    copy("settings.json")?;
    Ok(true)
}

/// The settings file for this launch, and whether its settings were just copied from before
/// 0.1.2. When the copy fails, this launch keeps its settings in another file, so settings.json
/// stays missing and the next launch tries the copy again.
fn settings_file(config_dir: &std::path::Path) -> (std::path::PathBuf, bool) {
    match copy_old_settings(config_dir) {
        Ok(copied) => (config_dir.join("settings.json"), copied),
        Err(e) => {
            eprintln!("SuperEditr: couldn't copy the settings from before 0.1.2 into {}: {e}", config_dir.display());
            (config_dir.join("settings-until-copied.json"), false)
        }
    }
}

/// Opening at login is on by default: turned on once, before Settings first reads the config, and
/// then left to the user. Only a copy of the app that stays where it is gets added, and while it's
/// on, each launch points the login item (a Launch Agent on macOS, an XDG autostart entry on Linux)
/// at the current copy, so moving the app doesn't break it.
fn launch_at_login_by_default(app: &AppHandle) {
    use tauri_plugin_autostart::ManagerExt;
    // The path the autostart plugin writes into the login item.
    let Some(exe) =
        std::env::current_exe().and_then(|exe| exe.canonicalize()).ok().map(|exe| exe.display().to_string())
    else {
        return;
    };
    if !lasting_app_path(&exe) {
        return;
    }
    let launcher = app.autolaunch();
    let state = app.state::<AppState>();
    let mut config = state.config();
    if config.offered_launch_at_login {
        // Rewritten only when the app has moved: macOS announces every change to a login item.
        let moved =
            login_item(app).is_some_and(|item| std::fs::read_to_string(item).is_ok_and(|text| !text.contains(&exe)));
        if moved && let Err(e) = launcher.enable() {
            eprintln!("SuperEditr: couldn't update opening at login: {e}");
        }
        return;
    }
    if let Err(e) = launcher.enable() {
        eprintln!("SuperEditr: couldn't turn on opening at login: {e}");
        return;
    }
    config.offered_launch_at_login = true;
    if let Err(e) = state.save_config(config) {
        eprintln!("SuperEditr: couldn't save settings: {e}");
    }
}

/// Whether the desktop setup is done, so Settings needn't open at launch (Linux's setup page).
#[cfg(target_os = "linux")]
fn desktop_set_up() -> bool {
    platform::setup::config_home().is_some_and(|config| platform::setup::complete(&config))
}

#[cfg(not(target_os = "linux"))]
fn desktop_set_up() -> bool {
    false
}

/// SuperEditr was started again while running. `supereditr review` and `supereditr quick-fix` act as
/// the shortcuts do, for a desktop's own key binding to run; anything else shows Settings.
fn on_second_launch(app: &AppHandle, args: &[String]) {
    match args.get(1).map(String::as_str) {
        Some("review") => review::on_review_hotkey(app),
        Some("quick-fix") => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { review::quick_fix(&app).await });
        }
        _ => show_settings(app, None),
    }
}

fn autostart() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    let builder = tauri_plugin_autostart::Builder::new();
    #[cfg(target_os = "macos")]
    let builder = builder.macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.app_name(AUTOSTART_NAME);
    builder.build()
}

pub fn run() {
    #[cfg(target_os = "linux")]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.get(1).map(String::as_str) == Some("uninstall-omarchy") {
            std::process::exit(platform::setup::uninstall_command(&args[2..]));
        }
    }
    let mut builder = tauri::Builder::default();
    if updater::ENABLED {
        builder = builder.plugin(updater::plugin());
    }
    // On Linux the compositor owns global keys (see `hotkeys`).
    #[cfg(not(target_os = "linux"))]
    {
        builder = builder.plugin(hotkeys::plugin());
    }
    builder
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| on_second_launch(app, &args)))
        .plugin(tauri_plugin_opener::init())
        .plugin(autostart())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                app.handle().plugin(tauri_nspanel::init())?;
            }
            let config_dir = app.path().app_config_dir()?;
            let (settings, copied) = settings_file(&config_dir);
            let install = config_dir.join("install.json");
            if let Err(e) = Install::record(&install, env!("CARGO_PKG_VERSION"), settings.exists()) {
                eprintln!("SuperEditr: couldn't record the first run in {}: {e}", install.display());
            }
            app.manage(AppState::new(settings));
            if copied {
                // Before anything reads a key, so the copies are what it finds.
                let providers = app.state::<AppState>().config().providers;
                secrets::copy_old(providers.iter().map(|p| p.id.as_str()));
            }
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
            #[cfg(target_os = "linux")]
            {
                // Before the shortcuts are registered: they go through the desktop this finds.
                let shortcut_app = handle.clone();
                platform::start(move |action| hotkeys::on_linux_shortcut(&shortcut_app, action));
                let switch_app = handle.clone();
                platform::start_app_switch_watcher(move |pid| {
                    review::on_app_switch(&switch_app, pid);
                    app_toggle::on_app_switch(&switch_app, pid);
                });
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
                platform::start_app_switch_watcher(move |pid| {
                    review::on_app_switch(&switch_app, pid);
                    app_toggle::on_app_switch(&switch_app, pid);
                });
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
            // Settings opens by itself on the first run, until Get Started is done or, on Linux,
            // the desktop is set up (SuperEditr then opens at login, so Settings would open at every
            // login); and on macOS while Accessibility isn't granted, since nothing works without it.
            let first_run = !handle.state::<AppState>().config().onboarded && !desktop_set_up();
            if first_run || (cfg!(target_os = "macos") && !platform::accessibility_trusted(false)) {
                show_settings(&handle, Some(if first_run { "welcome".into() } else { "privacy".into() }));
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
            commands::app_info,
            commands::pick_apps,
            commands::guide_accessibility,
            commands::close_guide,
            commands::drag_app,
            commands::preview_review,
            commands::set_paused,
            commands::linux_setup,
            commands::set_up_omarchy,
            commands::remove_omarchy_setup,
        ])
        .run(tauri::generate_context!())
        .expect("SuperEditr failed to start");
}

#[cfg(test)]
mod tests {
    use super::{OLD_IDENTIFIER, copy_old_settings, lasting_app_path, settings_file};

    #[test]
    fn an_update_keeps_the_settings_from_the_old_folder() {
        let root = std::env::temp_dir().join(format!("supereditr-old-settings-{}", std::process::id()));
        let old = root.join(OLD_IDENTIFIER);
        let new = root.join("app.supereditr");
        let read = |path: std::path::PathBuf| std::fs::read_to_string(path).unwrap();

        assert!(!copy_old_settings(&new).unwrap());
        assert!(!new.exists(), "a first install has nothing to copy");

        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("settings.json"), r#"{"onboarded": true}"#).unwrap();
        std::fs::write(old.join("install.json"), r#"{"first_version": "0.1.0"}"#).unwrap();
        std::fs::write(old.join("status.json"), "{}").unwrap();
        assert!(copy_old_settings(&new).unwrap());
        assert_eq!(read(new.join("settings.json")), r#"{"onboarded": true}"#);
        assert_eq!(read(new.join("install.json")), r#"{"first_version": "0.1.0"}"#);
        assert!(!new.join("status.json").exists(), "each launch writes its own status");
        assert!(!new.join("settings.json.tmp").exists());
        assert_eq!(read(old.join("settings.json")), r#"{"onboarded": true}"#, "the old folder stays");

        // Settings this version saved aren't replaced on a later launch.
        std::fs::write(new.join("settings.json"), r#"{"onboarded": true, "dialect": "en-GB"}"#).unwrap();
        assert!(!copy_old_settings(&new).unwrap());
        assert_eq!(read(new.join("settings.json")), r#"{"onboarded": true, "dialect": "en-GB"}"#);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_first_run_record_that_cant_be_read_doesnt_stop_the_settings() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("supereditr-old-record-{}", std::process::id()));
        let old = root.join(OLD_IDENTIFIER);
        let new = root.join("app.supereditr");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("settings.json"), r#"{"onboarded": true}"#).unwrap();
        std::fs::write(old.join("install.json"), "{}").unwrap();
        std::fs::set_permissions(old.join("install.json"), std::fs::Permissions::from_mode(0o000)).unwrap();
        assert!(copy_old_settings(&new).unwrap());
        assert_eq!(std::fs::read_to_string(new.join("settings.json")).unwrap(), r#"{"onboarded": true}"#);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_copy_is_tried_again_on_the_next_launch() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("supereditr-old-retry-{}", std::process::id()));
        let old = root.join(OLD_IDENTIFIER);
        let new = root.join("app.supereditr");
        std::fs::create_dir_all(&old).unwrap();
        let old_settings = old.join("settings.json");
        std::fs::write(&old_settings, r#"{"onboarded": true}"#).unwrap();
        std::fs::set_permissions(&old_settings, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&old_settings).is_ok() {
            // Running as root, which reads it anyway.
            std::fs::remove_dir_all(root).unwrap();
            return;
        }
        // This launch saves its settings elsewhere, defaults included.
        let (settings, copied) = settings_file(&new);
        assert!(!copied);
        assert_ne!(settings, new.join("settings.json"));
        std::fs::write(&settings, r#"{"onboarded": false}"#).unwrap();

        std::fs::set_permissions(&old_settings, std::fs::Permissions::from_mode(0o644)).unwrap();
        let (settings, copied) = settings_file(&new);
        assert!(copied, "the next launch copies the old settings");
        assert_eq!(settings, new.join("settings.json"));
        assert_eq!(std::fs::read_to_string(settings).unwrap(), r#"{"onboarded": true}"#);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn only_a_copy_that_stays_put_opens_at_login() {
        assert!(lasting_app_path("/Applications/SuperEditr.app/Contents/MacOS/supereditr"));
        assert!(lasting_app_path("/Users/me/Applications/SuperEditr.app/Contents/MacOS/supereditr"));
        assert!(!lasting_app_path("/Volumes/SuperEditr/SuperEditr.app/Contents/MacOS/supereditr"));
        assert!(!lasting_app_path(
            "/private/var/folders/xy/T/AppTranslocation/0A1B2C/d/SuperEditr.app/Contents/MacOS/supereditr"
        ));
        assert!(!lasting_app_path("/Users/me/supereditr/target/debug/supereditr"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn only_an_installed_copy_opens_at_login() {
        assert!(lasting_app_path("/usr/bin/supereditr"));
        assert!(lasting_app_path("/opt/supereditr/supereditr"));
        assert!(lasting_app_path("/home/rhyan/.local/bin/supereditr"));
        assert!(!lasting_app_path("/tmp/.mount_SuperEaBc12/usr/bin/supereditr"), "an AppImage's mount");
        assert!(!lasting_app_path("/home/rhyan/supereditr/target/release/supereditr"));
        assert!(!lasting_app_path("/home/rhyan/supereditr/target/debug/supereditr"));
        assert!(!lasting_app_path("/home/rhyan/My Apps/supereditr"), "a space needs quoting in Exec=");
        assert!(!lasting_app_path("supereditr"));
    }
}
