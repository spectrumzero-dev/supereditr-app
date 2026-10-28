//! Commands invoked by the overlay and settings windows.

use crate::state::AppState;
use crate::{hotkeys, overlay, platform, review, secrets};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Instant;
use supereditr_core::detect::{self, Candidate};
use supereditr_core::provider::{ChatRequest, ProviderKind, ProviderProfile, preferred_model};
use supereditr_core::{Config, Event, Job};
use tauri::{AppHandle, Emitter, Manager, State};

type CmdResult<T> = Result<T, String>;

// --- Overlay -------------------------------------------------------------------------------

#[tauri::command]
pub fn run_preset(app: AppHandle, id: u64, preset_id: String, steer: Option<String>) {
    review::run(&app, id, preset_id, steer);
}

#[tauri::command]
pub async fn apply_text(app: AppHandle, id: u64, text: String) -> CmdResult<String> {
    review::apply(&app, id, text).await
}

#[tauri::command]
pub fn dismiss(app: AppHandle, id: Option<u64>) {
    review::dismiss(&app, id);
}

#[tauri::command]
pub fn copy_text(text: String) {
    platform::copy_to_clipboard(&text);
}

#[tauri::command]
pub fn resize_overlay(app: AppHandle, height: f64) {
    review::resize(&app, height);
}

#[tauri::command]
pub fn focus_overlay(app: AppHandle, state: State<'_, AppState>) {
    if let Some(session) = state.session.lock().unwrap().as_mut() {
        session.keyboard = true;
    }
    overlay::show(&app, true);
}

/// The panel's instruction box opened (`true`) or closed. Synchronous, so it runs on the main
/// thread, and the panel has focus when the box asks for it.
#[tauri::command]
pub fn overlay_typing(app: AppHandle, typing: bool) {
    review::typing(&app, typing);
}

/// "Always keep" in the panel: adds a word to the author's word list. Returns false when the list
/// already has it.
#[tauri::command]
pub fn add_word(app: AppHandle, state: State<'_, AppState>, word: String) -> CmdResult<bool> {
    let mut config = state.config();
    if !supereditr_core::words::add(&mut config.words, &word) {
        return Ok(false);
    }
    state.save_config(config.clone()).map_err(|e| e.to_string())?;
    prepare_words(&state, config.words);
    let _ = app.emit_to("settings", "settings:reload", ());
    Ok(true)
}

/// Rebuilds the offline checker with a changed word list in the background, so the next review
/// doesn't wait for it.
fn prepare_words(state: &AppState, words: Vec<String>) {
    let engine = state.engine.clone();
    tauri::async_runtime::spawn_blocking(move || engine.prepare(&words));
}

#[tauri::command]
pub fn open_settings(app: AppHandle, section: Option<String>) {
    review::dismiss(&app, None);
    crate::show_settings(&app, section);
}

// --- Settings ------------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPayload {
    config: Config,
    /// Which provider profiles have a saved key.
    keys: HashMap<String, bool>,
    accessibility: bool,
    version: &'static str,
    platform: &'static str,
    hotkey_problems: Vec<String>,
}

#[tauri::command]
pub fn get_settings(app: AppHandle, state: State<'_, AppState>) -> SettingsPayload {
    let config = state.config();
    let started = std::time::Instant::now();
    let keys = config.providers.iter().map(|p| (p.id.clone(), state.key_for(&p.id).is_some())).collect();
    // A slow key read means macOS asked for the keychain password (a new build's first read).
    // Focus then returns to the previous app, so bring Settings back.
    // Settings loads hidden at launch, so only a visible window is brought back.
    let visible = app.get_webview_window("settings").and_then(|w| w.is_visible().ok()).unwrap_or(false);
    if visible && started.elapsed() > std::time::Duration::from_millis(500) {
        crate::show_settings(&app, None);
    }
    SettingsPayload {
        config,
        keys,
        accessibility: platform::accessibility_trusted(false),
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        hotkey_problems: Vec::new(),
    }
}

#[tauri::command]
pub fn save_settings(app: AppHandle, state: State<'_, AppState>, config: Config) -> CmdResult<Vec<String>> {
    if config.presets.is_empty() {
        return Err("Keep at least one style.".into());
    }
    let previous = state.config();
    state.save_config(config.clone()).map_err(|e| e.to_string())?;
    if previous.active_provider != config.active_provider {
        crate::show_ai_status(&app);
    }
    if previous.words != config.words {
        prepare_words(&state, config.words.clone());
    }
    if previous.hotkeys.review != config.hotkeys.review
        || previous.hotkeys.quick_fix != config.hotkeys.quick_fix
        || previous.presets.iter().map(|p| &p.shortcut).ne(config.presets.iter().map(|p| &p.shortcut))
    {
        return Ok(hotkeys::register_from_config(&app));
    }
    Ok(Vec::new())
}

#[tauri::command]
pub fn set_api_key(
    app: AppHandle,
    state: State<'_, AppState>,
    profile_id: String,
    key: Option<String>,
) -> CmdResult<()> {
    match key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) {
        Some(key) => {
            secrets::set(&profile_id, &key)?;
            state.set_cached_key(&profile_id, Some(key));
        }
        None => {
            secrets::delete(&profile_id);
            state.set_cached_key(&profile_id, None);
        }
    }
    if state.config().active_provider.as_deref() == Some(profile_id.as_str()) {
        crate::show_ai_status(&app);
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateView {
    index: usize,
    #[serde(flatten)]
    candidate: Candidate,
    label: &'static str,
    /// The provider already added from this account, if any, so the list can show it as in use
    /// (or switch to it) instead of adding it again.
    profile_id: Option<String>,
}

/// The provider already added from a detected account: same kind and the same key, or for
/// keyless kinds (local servers, the Claude CLI) the same address or path.
fn matching_profile(state: &AppState, candidate: &Candidate) -> Option<ProviderProfile> {
    state.config().providers.into_iter().find(|p| {
        p.kind == candidate.kind
            && match &candidate.api_key {
                Some(key) => state.key_for(&p.id).as_deref() == Some(key.as_str()),
                None => p.base_url == candidate.base_url,
            }
    })
}

#[tauri::command]
pub async fn scan_accounts(state: State<'_, AppState>) -> CmdResult<Vec<CandidateView>> {
    let home = std::env::var("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    // Apple Intelligence is built in and shown separately (see `builtin_ai`), not as an account.
    let mut found = detect::scan(&state.engine.providers, &home).await;
    found.retain(|c| c.kind != ProviderKind::AppleIntelligence);
    let views = found
        .iter()
        .enumerate()
        .map(|(index, c)| CandidateView {
            index,
            candidate: c.clone(),
            label: c.kind.display_name(),
            profile_id: matching_profile(&state, c).map(|p| p.id),
        })
        .collect();
    *state.candidates.lock().unwrap() = found;
    Ok(views)
}

/// Turns a detected account into a provider profile, picks its fastest model, and makes it
/// the active provider if it passes its check (otherwise it is added, and the error returned).
#[tauri::command]
pub async fn adopt_candidate(app: AppHandle, state: State<'_, AppState>, index: usize) -> CmdResult<ProviderProfile> {
    let candidate =
        state.candidates.lock().unwrap().get(index).cloned().ok_or("That result has expired. Scan again.")?;
    // Already added: switch to it rather than adding a duplicate.
    if let Some(existing) = matching_profile(&state, &candidate) {
        activate_provider(app, state.clone(), existing.id.clone()).await?;
        return Ok(ProviderProfile { api_key: None, ..existing });
    }
    let mut profile = ProviderProfile::new(candidate.kind);
    profile.base_url = candidate.base_url.clone();
    profile.api_key = candidate.api_key.clone();
    profile.id = unique_id(&state, candidate.kind);
    if let Ok(models) = state.engine.providers.list_models(&profile).await {
        if !models.is_empty() {
            profile.model = preferred_model(candidate.kind, &models);
        }
    } else if !candidate.models.is_empty() {
        profile.model = preferred_model(candidate.kind, &candidate.models);
    }
    if let Some(key) = &candidate.api_key {
        secrets::set(&profile.id, key)?;
        state.set_cached_key(&profile.id, Some(key.clone()));
    }
    let mut config = state.config();
    config.providers.push(ProviderProfile { api_key: None, ..profile.clone() });
    state.save_config(config).map_err(|e| e.to_string())?;
    // Added either way, but in use only if it works.
    activate_provider(app, state.clone(), profile.id.clone()).await?;
    Ok(ProviderProfile { api_key: None, ..profile })
}

/// Whether an AI is ready to review: its key is saved and its server answers (a model-list
/// request), or for the Claude subscription, the `claude` program runs.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCheck {
    ready: bool,
    /// What's wrong, in words the user can act on.
    problem: Option<String>,
    models: Vec<String>,
}

impl ProviderCheck {
    pub fn problem(&self) -> Option<&str> {
        if self.ready { None } else { Some(self.problem.as_deref().unwrap_or("not working")) }
    }
}

pub async fn check(state: &AppState, profile_id: &str) -> ProviderCheck {
    let Some(profile) = state.profile(profile_id) else {
        return ProviderCheck { ready: false, problem: Some("This AI was removed.".into()), models: Vec::new() };
    };
    match state.engine.providers.check(&profile).await {
        // The saved model must be one the server offers; activation picks one if it isn't.
        Ok(models) if !models.contains(&profile.model) => ProviderCheck {
            ready: false,
            problem: Some(if profile.model.is_empty() {
                "Choose a model".into()
            } else {
                format!("Choose a model: {} isn’t available", profile.model)
            }),
            models,
        },
        Ok(models) => ProviderCheck { ready: true, problem: None, models },
        Err(problem) => ProviderCheck { ready: false, problem: Some(problem), models: Vec::new() },
    }
}

/// Checks an AI. When it is the one in use, the menu-bar menu shows the result.
#[tauri::command]
pub async fn check_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    profile_id: String,
) -> CmdResult<ProviderCheck> {
    let result = check(&state, &profile_id).await;
    if state.config().active_provider.as_deref() == Some(profile_id.as_str()) {
        crate::show_ai_status(&app);
    }
    Ok(result)
}

/// Makes an AI the one in use, only if it passes its check: a misconfigured AI never becomes
/// active, so reviews keep using the one that works.
#[tauri::command]
pub async fn activate_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    profile_id: String,
) -> CmdResult<ProviderCheck> {
    let mut result = check(&state, &profile_id).await;
    // Reachable, but its saved model isn't offered: switch to the fastest one it has.
    if !result.ready && !result.models.is_empty() {
        let mut config = state.config();
        if let Some(p) = config.providers.iter_mut().find(|p| p.id == profile_id) {
            p.model = preferred_model(p.kind, &result.models);
            state.save_config(config).map_err(|e| e.to_string())?;
            result = check(&state, &profile_id).await;
        }
    }
    if !result.ready {
        return Err(result.problem.unwrap_or_else(|| "This AI isn’t working yet.".into()));
    }
    let mut config = state.config();
    config.active_provider = Some(profile_id);
    state.save_config(config).map_err(|e| e.to_string())?;
    crate::show_ai_status(&app);
    Ok(result)
}

fn unique_id(state: &AppState, kind: ProviderKind) -> String {
    let base = ProviderProfile::new(kind).id;
    let config = state.config();
    let mut id = base.clone();
    let mut n = 2;
    while config.providers.iter().any(|p| p.id == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

#[tauri::command]
pub fn add_provider(state: State<'_, AppState>, kind: ProviderKind) -> CmdResult<ProviderProfile> {
    let mut profile = ProviderProfile::new(kind);
    profile.id = unique_id(&state, kind);
    let mut config = state.config();
    // Not made active here: that waits for its key and a passing check (`activate_provider`).
    config.providers.push(profile.clone());
    state.save_config(config).map_err(|e| e.to_string())?;
    Ok(profile)
}

#[tauri::command]
pub fn remove_provider(app: AppHandle, state: State<'_, AppState>, profile_id: String) -> CmdResult<()> {
    let mut config = state.config();
    config.providers.retain(|p| p.id != profile_id);
    // Removing the AI in use returns reviews to the built-in one.
    if config.active_provider.as_deref() == Some(profile_id.as_str()) {
        config.active_provider = None;
    }
    secrets::delete(&profile_id);
    state.set_cached_key(&profile_id, None);
    state.save_config(config).map_err(|e| e.to_string())?;
    crate::show_ai_status(&app);
    Ok(())
}

#[tauri::command]
pub async fn list_models(state: State<'_, AppState>, profile_id: String) -> CmdResult<Vec<String>> {
    let profile = state.profile(&profile_id).ok_or("Unknown provider.")?;
    state.engine.providers.list_models(&profile).await.map_err(|e| e.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    first_token_ms: Option<u64>,
    total_ms: u64,
    output: String,
}

/// Sends a tiny proofreading request and reports latency.
#[tauri::command]
pub async fn test_provider(state: State<'_, AppState>, profile_id: String) -> CmdResult<TestResult> {
    let profile = state.profile(&profile_id).ok_or("Unknown provider.")?;
    let providers = &state.engine.providers;
    providers.warm(&profile).await;
    let request = ChatRequest {
        system: "Fix the spelling and grammar of the text in <text> tags. Reply with the corrected text only.".into(),
        user: "<text>This are a quick tset.</text>".into(),
        max_tokens: 64,
    };
    let started = Instant::now();
    let mut first = None;
    let output = providers
        .stream(&profile, &request, &mut |_| {
            first.get_or_insert(started.elapsed().as_millis() as u64);
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(TestResult {
        first_token_ms: first,
        total_ms: started.elapsed().as_millis() as u64,
        output: output.trim().to_string(),
    })
}

#[tauri::command]
pub async fn pull_ollama_model(
    app: AppHandle,
    state: State<'_, AppState>,
    base_url: String,
    model: String,
) -> CmdResult<()> {
    state
        .engine
        .providers
        .ollama_pull(&base_url, &model, &mut |progress| {
            let _ = app.emit_to("settings", "settings:pull", &progress);
        })
        .await
        .map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct BuiltinAi {
    /// False on Macs that can never run Apple Intelligence; the setup screens then hide it.
    supported: bool,
    available: bool,
    /// Why it can't be used yet, in words the user can act on.
    message: Option<String>,
}

/// Why a shortcut being recorded for `slot` can't be used, or `None` if it's free. See
/// `shortcut_check`.
#[tauri::command]
pub fn check_shortcut(app: AppHandle, state: State<'_, AppState>, accelerator: String, slot: String) -> Option<String> {
    let config = state.config();
    let others = hotkeys::others(&config, &slot);
    if let Some(problem) =
        crate::shortcut_check::conflict(&accelerator, &others, &crate::shortcut_check::user_hotkeys())
    {
        return Some(problem);
    }
    hotkeys::taken_by_another_app(&app, &accelerator).then(|| "Another app already uses this shortcut.".into())
}

/// The built-in styles as first installed, for resetting the Styles page.
#[tauri::command]
pub fn default_presets() -> Vec<supereditr_core::prompts::Preset> {
    supereditr_core::prompts::builtin_presets()
}

/// Apple Intelligence, the AI SuperEditr uses unless another is chosen.
#[tauri::command]
pub async fn builtin_ai(state: State<'_, AppState>) -> CmdResult<BuiltinAi> {
    let status = state.engine.providers.apple_status(&ProviderProfile::new(ProviderKind::AppleIntelligence)).await;
    let unsupported = matches!(status.reason.as_deref(), Some("osTooOld" | "deviceNotEligible" | "missing"));
    Ok(BuiltinAi { supported: !unsupported, available: status.available, message: status.message() })
}

#[tauri::command]
pub fn open_apple_intelligence_settings() {
    #[cfg(target_os = "macos")]
    let _ =
        std::process::Command::new("open").arg("x-apple.systempreferences:com.apple.Siri-Settings.extension").spawn();
}

/// Reviews the text in Settings' "Try it now" box in the real suggestion panel, which opens below
/// the box. `rect` is the box's frame inside the window, in CSS pixels (points). It is turned into
/// screen points from the window's own frame: the web view's idea of the window's screen position
/// (window.screenX/screenY) is wrong in this app. Synchronous, so it runs on the main thread.
#[tauri::command]
pub fn review_playground(app: AppHandle, window: tauri::WebviewWindow, text: String, rect: platform::Rect) {
    let mut bounds = rect;
    #[cfg(target_os = "macos")]
    if let Ok(ptr) = window.ns_window() {
        let ns_window = unsafe { &*(ptr as *const objc2_app_kit::NSWindow) };
        let frame = ns_window.frame();
        // AppKit's origin is the primary display's bottom-left; the web view fills the window,
        // title bar included (overlay title bar).
        let primary = core_graphics::display::CGDisplay::main().bounds().size.height;
        bounds.x += frame.origin.x;
        bounds.y += primary - (frame.origin.y + frame.size.height);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
    review::review_playground(&app, text, bounds);
}

/// Sets the Settings window's height in points, keeping its top edge where it is, so the welcome
/// page needs no empty space or scrolling. Clamped to fit the screen. Synchronous, so it runs on
/// the main thread, where AppKit requires it.
#[tauri::command]
pub fn fit_settings_height(window: tauri::WebviewWindow, height: f64) {
    #[cfg(target_os = "macos")]
    if let Ok(ptr) = window.ns_window() {
        let ns_window = unsafe { &*(ptr as *const objc2_app_kit::NSWindow) };
        let mut frame = ns_window.frame();
        let available = ns_window.screen().map(|s| s.visibleFrame().size.height).unwrap_or(900.0);
        let height = height.clamp(480.0, available);
        if (frame.size.height - height).abs() < 1.0 {
            return;
        }
        frame.origin.y += frame.size.height - height;
        frame.size.height = height;
        ns_window.setFrame_display(frame, true);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (window, height);
}

/// Turns SuperEditr's shortcuts off while Settings records one (see `hotkeys::pause`).
#[tauri::command]
pub fn pause_shortcuts(app: AppHandle, paused: bool) {
    hotkeys::pause(&app, paused);
}

#[tauri::command]
pub fn accessibility_status(prompt: bool) -> bool {
    platform::accessibility_trusted(prompt)
}

#[tauri::command]
pub fn open_accessibility_settings() {
    platform::open_accessibility_settings();
}

/// Opens the Accessibility pane with the drag-and-drop guide beside it.
#[tauri::command]
pub fn guide_accessibility(app: AppHandle) {
    crate::permission_guide::start(&app);
}

#[tauri::command]
pub fn close_guide(app: AppHandle) {
    crate::permission_guide::dismiss(&app);
}

/// Synchronous so it runs on the main thread while the mouse is still down.
#[tauri::command]
pub fn drag_app(window: tauri::WebviewWindow) -> CmdResult<()> {
    crate::permission_guide::drag_app(&window)
}

#[derive(Clone, Serialize)]
struct PreviewPayload {
    id: u64,
    event: Event,
}

/// Runs a review on text typed into the Settings playground, so setup can be tested without
/// leaving the window.
#[tauri::command]
pub async fn preview_review(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u64,
    text: String,
    preset_id: String,
) -> CmdResult<()> {
    let config = state.config();
    let preset = config.preset(&preset_id).cloned().unwrap_or_else(|| config.default_preset().clone());
    let job = Job {
        text,
        preset,
        steer: None,
        profile: state.active_profile(),
        style_notes: config.style_notes.clone(),
        dialect: config.dialect.clone(),
        words: config.words.clone(),
        local_checks: true,
        builtin_ai: true,
    };
    state
        .engine
        .run(job, &mut |event| {
            let _ = app.emit_to("settings", "settings:preview", PreviewPayload { id, event });
        })
        .await;
    Ok(())
}

#[tauri::command]
pub fn set_paused(app: AppHandle, paused: bool) {
    app.state::<AppState>().paused.store(paused, std::sync::atomic::Ordering::Relaxed);
}
