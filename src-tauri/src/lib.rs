//! SuperEditr desktop shell: menu-bar app, global shortcuts, overlay, and settings.

#[cfg(test)]
mod accelerator_contract;
mod app_toggle;
mod apps;
mod commands;
mod double_press;
mod follow;
mod hotkeys;
#[cfg(test)]
mod ipc_contract;
#[cfg_attr(not(windows), allow(dead_code))]
mod keys;
mod last_review;
mod login_item;
mod overlay;
mod permission_guide;
mod platform;
mod review;
mod review_keys;
mod secrets;
#[cfg_attr(windows, allow(dead_code))]
mod shortcut_check;
mod state;
#[cfg(target_os = "linux")]
mod tray_linux;
#[cfg(test)]
mod ui_wording_contract;
mod updater;

use state::AppState;
#[cfg(not(target_os = "linux"))]
use std::sync::atomic::Ordering;
use supereditr_core::config::ConfigFile;
use supereditr_core::install::Install;
use supereditr_core::provider::{ProviderKind, ProviderProfile};
#[cfg(not(target_os = "linux"))]
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
#[cfg(not(target_os = "linux"))]
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

/// Keeps `status.json` (next to settings.json) current with whether macOS trusts SuperEditr for
/// Accessibility. Only the app itself can ask, so this is how scripts and UI tests check it. On
/// Linux it also holds the shortcuts in use and which of them the desktop left unbound, for the
/// Omarchy bar widget.
fn write_status_file(app: AppHandle, path: std::path::PathBuf) {
    std::thread::spawn(move || {
        let mut last = None;
        loop {
            let trusted = platform::accessibility_trusted(false);
            let current = (trusted, shortcut_status(&app));
            if last.as_ref() != Some(&current) {
                let mut status = serde_json::json!({
                    "accessibility": trusted,
                    "version": env!("CARGO_PKG_VERSION"),
                    "pid": std::process::id(),
                    "updated": std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                });
                if let Some(shortcuts) = &current.1 {
                    status["shortcuts"] = shortcuts.clone();
                }
                let tmp = path.with_extension("json.tmp");
                if std::fs::write(&tmp, status.to_string()).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
                    last = Some(current);
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}

/// The review and quick-fix shortcuts as settings name them ("" when off), and which of them the
/// desktop left unbound because something else uses the key: `{"review": "Super+Alt+E",
/// "quick_fix": "…", "refused": ["quick_fix"]}`. The settings file can be older than what the app
/// uses, since the app moves an old default to the new one only in memory.
#[cfg(target_os = "linux")]
fn shortcut_status(app: &AppHandle) -> Option<serde_json::Value> {
    use platform::desktop::Action as DesktopAction;
    let hotkeys = app.try_state::<state::AppState>()?.config().hotkeys;
    let refused: Vec<&str> = platform::refused_shortcuts()
        .iter()
        .map(|(action, _)| match action {
            DesktopAction::Review => "review",
            DesktopAction::QuickFix => "quick_fix",
        })
        .collect();
    Some(serde_json::json!({ "review": hotkeys.review, "quick_fix": hotkeys.quick_fix, "refused": refused }))
}

#[cfg(not(target_os = "linux"))]
fn shortcut_status(_app: &AppHandle) -> Option<serde_json::Value> {
    None
}

pub fn show_settings(app: &AppHandle, section: Option<String>) {
    if let Some(window) = app.get_webview_window("settings") {
        platform::set_in_dock(app, true);
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
/// Omarchy's bar is dark, so Linux gets a light one. Windows shows app icons in full colour in the
/// notification area, on a light or dark taskbar, so it gets the app icon.
#[cfg(not(any(target_os = "linux", windows)))]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray.png");
#[cfg(target_os = "linux")]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray-light.png");
#[cfg(windows)]
const TRAY_ICON: &[u8] = include_bytes!("../icons/32x32.png");

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
    if updater::available() {
        // Below Settings…
        menu.insert(&updater::menu_item(app)?, 7)?;
    }
    app_toggle::init(app, &menu, app_toggle)?;
    let icon = tauri::image::Image::from_bytes(TRAY_ICON)?;
    let pause_item = pause.clone();
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
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
            "quit" => {
                // A downloaded update installs as SuperEditr quits, without starting it again.
                #[cfg(windows)]
                updater::install(app, false);
                app.exit(0)
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

/// The file that opens SuperEditr at login, as the autostart plugin writes it.
#[cfg(not(windows))]
fn login_item(app: &AppHandle) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    return app.path().home_dir().ok().map(|home| home.join("Library/LaunchAgents/SuperEditr.plist"));
    #[cfg(not(target_os = "macos"))]
    return app.path().config_dir().ok().map(|config| config.join(format!("autostart/{AUTOSTART_NAME}.desktop")));
}

/// Whether the login item opens a copy of the app other than `exe`, so it needs rewriting.
#[cfg(not(windows))]
fn login_item_moved(app: &AppHandle, exe: &str) -> bool {
    login_item(app).is_some_and(|item| std::fs::read_to_string(item).is_ok_and(|text| !text.contains(exe)))
}

/// On Windows the login item is a Run key value, and rewriting it is silent, so while it's on it's
/// rewritten at every launch. Turned off, in Settings or Task Manager, it's left alone.
#[cfg(windows)]
fn login_item_moved(app: &AppHandle, _exe: &str) -> bool {
    login_item::is_enabled(app).unwrap_or(false)
}

/// The autostart entry's name: `~/.config/autostart/supereditr.desktop` on Linux. Windows writes
/// its Run value itself (`platform::login`).
#[cfg(target_os = "linux")]
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
    use std::io::{self, ErrorKind, Write};
    let old = config_dir.with_file_name(OLD_IDENTIFIER);
    let current = config_dir.join("settings.json");
    if !matches!(std::fs::symlink_metadata(&current), Err(e) if e.kind() == ErrorKind::NotFound) {
        return Ok(false);
    }
    let legacy = old.join("settings.json");
    let bytes = match std::fs::read(&legacy) {
        Err(e)
            if e.kind() == ErrorKind::NotFound
                && matches!(std::fs::symlink_metadata(&legacy), Err(e) if e.kind() == ErrorKind::NotFound) =>
        {
            return Ok(false);
        }
        result => result?,
    };
    if !serde_json::from_slice::<serde_json::Value>(&bytes).is_ok_and(|value| value.is_object()) {
        return Err(io::Error::new(ErrorKind::InvalidData, "the previous settings file is not a JSON object"));
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
    let mut file = tempfile::NamedTempFile::new_in(config_dir)?;
    file.write_all(&bytes)?;
    file.as_file().sync_all()?;
    // A file created since the check above belongs to the current version.
    match file.persist_noclobber(&current) {
        Ok(_) => Ok(true),
        Err(e) if e.error.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e.error),
    }
}

/// Load the current file, checking the previous location before treating it as a first run.
fn load_app_settings(path: &std::path::Path) -> ConfigFile {
    match copy_old_settings(path.parent().expect("settings have a config directory")) {
        Ok(_) => ConfigFile::load(path),
        Err(e) => {
            eprintln!("SuperEditr: couldn't copy the settings from before 0.1.2 into {}: {e}", path.display());
            ConfigFile::unavailable(
                "SuperEditr couldn’t copy the settings from its previous folder. Check the file in the old com.sghael.supereditr folder and retry.",
            )
        }
    }
}

/// Opening at login is on by default: turned on once, before Settings first reads the config, and
/// then left to the user. Only a copy of the app that stays where it is gets added, and while it's
/// on, each launch points the login item (a Launch Agent on macOS, an XDG autostart entry on Linux)
/// at the current copy, so moving the app doesn't break it.
fn launch_at_login_by_default(app: &AppHandle) {
    let state = app.state::<AppState>();
    if !state.allow_startup_settings_changes() {
        return;
    }
    // The path the autostart plugin writes into the login item.
    let Some(exe) = std::env::current_exe()
        .and_then(|exe| if cfg!(windows) { Ok(exe) } else { exe.canonicalize() })
        .ok()
        .map(|exe| exe.display().to_string())
    else {
        return;
    };
    if !platform::lasting_app_path(&exe) {
        return;
    }
    if state.config().offered_launch_at_login {
        // Rewritten only when the app has moved: macOS announces every change to a login item.
        if login_item_moved(app, &exe)
            && let Err(e) = login_item::enable(app)
        {
            eprintln!("SuperEditr: couldn't update opening at login: {e}");
        }
        return;
    }
    if let Err(e) = login_item::enable(app) {
        eprintln!("SuperEditr: couldn't turn on opening at login: {e}");
        return;
    }
    platform::reload_autostart();
    if let Err(e) = state.update_config(None, |_, config| {
        config.offered_launch_at_login = true;
        Some(())
    }) {
        eprintln!("SuperEditr: couldn't save settings: {e}");
    }
}

/// SuperEditr was started again while running. `supereditr review` and `supereditr quick-fix` act as
/// the shortcuts do, for a desktop's own key binding to run; anything else shows Settings.
/// Runs the app's commands, marking each as running on the main thread while it does: the watchdog
/// names it when the main thread stops answering.
fn named_commands(
    handler: impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    move |invoke| {
        #[cfg(target_os = "linux")]
        let _running = platform::watchdog::running_command(invoke.message.command());
        handler(invoke)
    }
}

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

/// Sizes the Settings window for the screen, and keeps it when the user closes it.
fn prepare_settings(app: &AppHandle, settings: &tauri::WebviewWindow) {
    // A tiling desktop (Omarchy's Hyprland) sizes Settings itself, often to half the screen. Below
    // the window's minimum width, GTK would draw the page wider than the window and cut off its
    // right side.
    #[cfg(target_os = "linux")]
    let _ = settings.set_min_size(Some(tauri::LogicalSize::new(420.0, 400.0)));
    #[cfg(windows)]
    commands::fit_on_screen(settings, None);
    let window = settings.clone();
    let app = app.clone();
    settings.on_window_event(move |event| {
        // A review of the "Try it now" box follows the window, and closes with it.
        follow::settings_event(&app, &window, event);
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = window.hide();
            platform::set_in_dock(&app, false);
        }
    });
}

/// Starts a window's page again when its WebView2 processes stop (`platform::webview`): a page whose
/// renderer stopped has been loaded again, and a window whose browser process stopped is made again.
/// The panel's review ends either way, since its page starts empty.
#[cfg(windows)]
fn watch_webview(app: &AppHandle, window: &tauri::WebviewWindow) {
    let (app, label) = (app.clone(), window.label().to_string());
    platform::webview::on_stopped(window, move |stopped| {
        let process = match stopped {
            platform::webview::Stopped::Browser => platform::log_lines::BROWSER,
            platform::webview::Stopped::Renderer => platform::log_lines::RENDERER,
        };
        platform::trace(format_args!("{}", platform::log_lines::WebviewStopped { window: &label, process }));
        if label == "overlay" {
            review::dismiss(&app, None);
        }
        if stopped == platform::webview::Stopped::Browser {
            remake_window(&app, &label);
        }
    });
}

/// Makes a window again from its configuration, on a thread of its own: the old one is destroyed
/// first, and its label is free only once the main thread has done that. A window that keeps failing
/// is made again at most a few times in ten minutes, so a broken WebView2 doesn't make SuperEditr
/// loop.
#[cfg(windows)]
fn remake_window(app: &AppHandle, label: &str) {
    use std::time::{Duration, Instant};
    static REMADE: std::sync::Mutex<Vec<(String, Instant)>> = std::sync::Mutex::new(Vec::new());
    {
        let mut remade = REMADE.lock().unwrap_or_else(|p| p.into_inner());
        remade.retain(|(_, at)| at.elapsed() < Duration::from_secs(600));
        if remade.iter().filter(|(l, _)| l == label).count() >= 3 {
            return platform::trace(format_args!("webview: not making the {label} window again: it keeps failing"));
        }
        remade.push((label.to_string(), Instant::now()));
    }
    let Some(config) = app.config().app.windows.iter().find(|w| w.label == label).cloned() else { return };
    let (app, label) = (app.clone(), label.to_string());
    std::thread::spawn(move || {
        // One window at a time: making a webview waits on the main thread for WebView2, and after the
        // browser process stopped, windows made side by side there never finished.
        static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
        let shown = app.get_webview_window(&label).is_some_and(|old| {
            let shown = old.is_visible().unwrap_or(false);
            let _ = old.destroy();
            shown
        });
        let started = Instant::now();
        while app.get_webview_window(&label).is_some() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let window = match tauri::WebviewWindowBuilder::from_config(&app, &config).and_then(|b| b.build()) {
            Ok(window) => window,
            Err(error) => {
                return platform::trace(format_args!("webview: couldn't make the {label} window again: {error}"));
            }
        };
        watch_webview(&app, &window);
        match label.as_str() {
            // Its window procedure is subclassed, which only the window's own thread can do.
            "overlay" => {
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || overlay::init(&handle));
            }
            "settings" => {
                prepare_settings(&app, &window);
                if shown {
                    show_settings(&app, None);
                }
            }
            _ => {}
        }
        platform::trace(format_args!(
            "{}",
            platform::log_lines::WebviewMadeAgain { window: &label, took: started.elapsed() }
        ));
    });
}

#[cfg(not(windows))]
fn autostart() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    let builder = tauri_plugin_autostart::Builder::new();
    #[cfg(target_os = "macos")]
    let builder = builder.macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.app_name(AUTOSTART_NAME);
    builder.build()
}

pub fn run() {
    // The uninstaller runs this when "Delete app data" is ticked, before it removes the program.
    #[cfg(windows)]
    if std::env::args().nth(1).as_deref() == Some("delete-saved-keys") {
        std::process::exit(match secrets::delete_all() {
            Ok(_) => 0,
            Err(e) => {
                eprintln!("SuperEditr: couldn't delete the saved keys: {e}");
                1
            }
        });
    }
    // A mode of the program that runs instead of the app (`platform::run_helper_command`).
    if let Some(code) = platform::run_helper_command() {
        std::process::exit(code);
    }
    let mut builder = tauri::Builder::default();
    if updater::available() {
        builder = builder.plugin(updater::plugin());
    }
    // On Linux the compositor owns global keys, and Windows registers them itself (see `hotkeys`).
    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(hotkeys::plugin());
    }
    let mut context = tauri::generate_context!();
    updater::use_test_feed(&mut context);
    let builder = builder
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| on_second_launch(app, &args)))
        .plugin(tauri_plugin_opener::init());
    #[cfg(not(windows))]
    let builder = builder.plugin(autostart());
    builder
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                app.handle().plugin(tauri_nspanel::init())?;
            }
            let config_dir = app.path().app_config_dir()?;
            let settings = config_dir.join("settings.json");
            let install = config_dir.join("install.json");
            let loaded = load_app_settings(&settings);
            let state = AppState::with_settings(settings, loaded);
            if state.allow_startup_settings_changes()
                && let Err(e) = Install::record(&install, env!("CARGO_PKG_VERSION"), state.config_path.exists())
            {
                eprintln!("SuperEditr: couldn't record the first run in {}: {e}", install.display());
            }
            app.manage(state);
            launch_at_login_by_default(app.handle());
            app.manage(hotkeys::Registry::default());
            // The windows are made here, after the state their pages' commands read, rather than by
            // Tauri before `setup`: on Windows, making a webview runs a message loop that can deliver
            // a command from a page that has already loaded.
            for window in app.config().app.windows.clone() {
                let builder = tauri::WebviewWindowBuilder::from_config(app.handle(), &window)?;
                // On Linux the desktop sizes the panel (`platform::place_overlay`). A GTK window
                // that can't be resized tells the desktop the size it was made with is its largest,
                // and Hyprland keeps it to that: a taller panel was cut off at that height.
                #[cfg(target_os = "linux")]
                let builder = if window.label == "overlay" { builder.resizable(true) } else { builder };
                let _window = builder.build()?;
                #[cfg(windows)]
                watch_webview(app.handle(), &_window);
            }
            write_status_file(app.handle().clone(), config_dir.join("status.json"));

            let handle = app.handle().clone();
            overlay::init(&handle);
            permission_guide::init(&handle);
            if std::env::var_os("SUPEREDITR_PREVIEW_GUIDE").is_some() {
                permission_guide::start(&handle);
            }
            build_tray(&handle)?;
            show_ai_status(&handle);
            if updater::checks_automatically() {
                updater::start(&handle);
            }
            #[cfg(target_os = "linux")]
            {
                // Before the shortcuts are registered: they go through the desktop this finds.
                let shortcut_app = handle.clone();
                platform::start(move |action| hotkeys::on_linux_shortcut(&shortcut_app, action));
                platform::watchdog::start(&handle);
                let switch_app = handle.clone();
                platform::start_app_switch_watcher(move |pid| {
                    review::on_app_switch(&switch_app, pid);
                    app_toggle::on_app_switch(&switch_app, pid);
                });
                let space_app = handle.clone();
                platform::start_workspace_watcher(move || review::on_space_change(&space_app));
            }
            #[cfg(windows)]
            {
                // Before the shortcuts are registered: they go through the thread this starts.
                let events = handle.clone();
                platform::start(move |event| match event {
                    platform::hotkey::Event::Hotkey(id) => hotkeys::on_windows_hotkey(&events, id),
                    platform::hotkey::Event::Foreground(pid) => {
                        review::on_app_switch(&events, pid as i32);
                        app_toggle::on_app_switch(&events, pid as i32);
                    }
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
                let space_app = handle.clone();
                platform::start_space_change_watcher(move || review::on_space_change(&space_app));
            }

            if let Some(settings) = handle.get_webview_window("settings") {
                prepare_settings(&handle, &settings);
            }
            // Settings opens by itself on the first run, until Get Started is done or, on Linux,
            // the desktop is set up (SuperEditr then opens at login, so Settings would open at every
            // login); and on macOS while Accessibility isn't granted, since nothing works without it.
            let state = handle.state::<AppState>();
            let first_run = !state.config().onboarded && !platform::desktop_set_up();
            if !state.allow_startup_settings_changes() {
                show_settings(&handle, Some("privacy".into()));
            } else if first_run || (cfg!(target_os = "macos") && !platform::accessibility_trusted(false)) {
                show_settings(&handle, Some(if first_run { "welcome".into() } else { "privacy".into() }));
            }
            Ok(())
        })
        .invoke_handler(named_commands(tauri::generate_handler![
            commands::run_preset,
            commands::apply_text,
            commands::dismiss,
            commands::copy_text,
            commands::resize_overlay,
            commands::focus_overlay,
            commands::overlay_typing,
            commands::open_settings,
            commands::add_word,
            commands::hide_tips,
            commands::get_settings,
            commands::save_settings,
            commands::set_api_key,
            commands::scan_accounts,
            commands::adopt_candidate,
            commands::add_provider,
            commands::remove_provider,
            commands::test_provider,
            commands::pull_ollama_model,
            commands::builtin_ai,
            commands::default_presets,
            commands::check_shortcut,
            commands::check_provider,
            commands::claude_sign_in,
            commands::activate_provider,
            commands::review_playground,
            commands::fit_settings_height,
            commands::pause_shortcuts,
            commands::open_apple_intelligence_settings,
            commands::launch_at_login,
            commands::set_launch_at_login,
            commands::accessibility_status,
            commands::app_info,
            commands::password_managers,
            commands::pick_apps,
            commands::guide_accessibility,
            commands::close_guide,
            commands::drag_app,
            commands::update_status,
            commands::update_now,
            commands::close_update_window,
            commands::linux_setup,
            commands::set_up_omarchy,
            commands::remove_omarchy_setup,
        ]))
        .run(context)
        .expect("SuperEditr failed to start");
}

#[cfg(test)]
mod tests {
    use super::{OLD_IDENTIFIER, copy_old_settings};
    use crate::platform::lasting_app_path;

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
    fn a_failed_copy_is_read_only_until_it_can_be_retried() {
        use super::load_app_settings;
        use std::os::unix::fs::PermissionsExt;
        use supereditr_core::config::LoadStatus;
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
        let settings = new.join("settings.json");
        assert_eq!(load_app_settings(&settings).status(), LoadStatus::Unreadable);
        assert!(!new.exists(), "the failed migration writes nothing");

        std::fs::set_permissions(&old_settings, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(load_app_settings(&settings).status(), LoadStatus::Clean);
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

    #[cfg(windows)]
    #[test]
    fn only_an_installed_copy_opens_at_login() {
        assert!(lasting_app_path(r"C:\Users\rhyan\AppData\Local\SuperEditr\supereditr.exe"), "the installer's folder");
        assert!(lasting_app_path(r"\\?\C:\Users\rhyan\AppData\Local\SuperEditr\supereditr.exe"));
        assert!(lasting_app_path(r"D:\Apps\SuperEditr\supereditr.exe"));
        assert!(lasting_app_path(r"C:\Users\Rhyan Brynlee\AppData\Local\SuperEditr\supereditr.exe"));
        assert!(!lasting_app_path(r"C:\Users\rhyan\supereditr\target\release\supereditr.exe"));
        assert!(!lasting_app_path(r"C:\Users\rhyan\supereditr\target\debug\supereditr.exe"));
        assert!(!lasting_app_path(r"C:\Users\rhyan\AppData\Local\Temp\7zO1A2B\supereditr.exe"), "run from an archive");
        assert!(!lasting_app_path(r"C:\Windows\Temp\supereditr.exe"));
        assert!(!lasting_app_path(
            r"C:\Program Files\WindowsApps\SuperEditr_0.1.5.0_x64__k7cd5y853dpye\supereditr.exe"
        ));
        assert!(!lasting_app_path(r"\\quillpoint\apps\supereditr.exe"), "a network share");
        assert!(!lasting_app_path("supereditr.exe"));
    }

    #[cfg(not(any(target_os = "macos", windows)))]
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
