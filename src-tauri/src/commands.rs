//! Commands invoked by the overlay and settings windows.

use crate::state::AppState;
use crate::{apps, hotkeys, overlay, platform, review, secrets};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Instant;
use supereditr_core::Config;
use supereditr_core::detect::{self, Candidate};
use supereditr_core::policy::{Os, is_password_manager};
use supereditr_core::provider::{
    ChatRequest, CheckCode, CheckProblem, ProviderKind, ProviderProfile, Providers, PullProgress, preferred_model,
};
use tauri::{AppHandle, Emitter, Manager, State};

type CmdResult<T> = Result<T, String>;

#[cfg(test)]
mod tests;

// --- Overlay -------------------------------------------------------------------------------

#[tauri::command]
pub fn run_preset(app: AppHandle, id: u64, run: u64, preset_id: String, steer: Option<String>, fresh: bool) {
    review::run(&app, id, run, preset_id, steer, fresh);
}

#[tauri::command]
pub async fn apply_text(app: AppHandle, id: u64, text: String, anyway: Option<bool>) -> CmdResult<String> {
    review::apply(&app, id, text, anyway.unwrap_or(false)).await
}

#[tauri::command]
pub fn dismiss(app: AppHandle, id: Option<u64>) {
    review::dismiss(&app, id);
}

#[tauri::command]
pub fn copy_text(text: String) {
    platform::trace(format_args!("panel: copying {} characters", text.chars().count()));
    platform::copy_to_clipboard(&text);
}

#[tauri::command]
pub fn resize_overlay(app: AppHandle, height: f64) {
    review::resize(&app, height);
}

#[tauri::command]
pub fn focus_overlay(app: AppHandle, state: State<'_, AppState>) {
    state.review.lock().unwrap().take_keyboard();
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

/// The panel's key tips were closed: turns "always show" off, so they stay closed.
#[tauri::command]
pub fn hide_tips(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    let mut config = state.config();
    if config.always_show_tips {
        config.always_show_tips = false;
        state.save_config(config).map_err(|e| e.to_string())?;
        let _ = app.emit_to("settings", "settings:reload", ());
    }
    Ok(())
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
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SettingsPayload {
    config: Config,
    /// Which provider profiles have a saved key.
    keys: HashMap<String, bool>,
    accessibility: bool,
    version: &'static str,
    platform: &'static str,
    /// Where capture.log is written.
    log_path: Option<String>,
    hotkey_problems: Vec<String>,
    /// What the keys saved by virtual key ("VkBA") show on the current keyboard layout, such as Ü.
    key_labels: HashMap<String, String>,
    /// The revision `config` was saved at, for `save_settings`.
    revision: u64,
}

/// What became of a save from Settings.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct Saved {
    /// The settings' revision after this save, or None when the save was refused because they were
    /// saved elsewhere after the revision Settings sent. Settings then reloads and tries again.
    revision: Option<u64>,
    /// Shortcuts that couldn't be registered.
    problems: Vec<String>,
}

#[tauri::command]
pub fn get_settings(app: AppHandle, state: State<'_, AppState>) -> SettingsPayload {
    let (config, revision) = state.config_at_revision();
    let started = std::time::Instant::now();
    let keys =
        config.providers.iter().map(|p| (p.id.clone(), state.profile_key(p).is_ok_and(|key| key.is_some()))).collect();
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
        version: env!("SUPEREDITR_VERSION_LABEL"),
        platform: std::env::consts::OS,
        log_path: platform::log_path().map(|path| path.display().to_string()),
        hotkey_problems: refused_shortcuts(),
        #[cfg(windows)]
        key_labels: platform::key_labels(),
        #[cfg(not(windows))]
        key_labels: HashMap::new(),
        revision,
    }
}

/// Saves the whole config. With `revision`, only if nothing else has saved since that revision.
#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    config: Config,
    revision: Option<u64>,
) -> CmdResult<Saved> {
    let Some((revision, effects)) = save_from_settings(&state, config.clone(), revision)? else {
        return Ok(Saved { revision: None, problems: Vec::new() });
    };
    let revision = Some(revision);
    if effects.show_ai_status {
        crate::show_ai_status(&app);
    }
    if effects.prepare_words {
        prepare_words(&state, config.words);
    }
    if effects.refresh_app_toggle {
        crate::app_toggle::refresh(&app);
    }
    if effects.register_shortcuts {
        return Ok(Saved { revision, problems: hotkeys::register_from_config(&app) });
    }
    // The shortcuts weren't touched, so any the desktop refused still are.
    Ok(Saved { revision, problems: refused_shortcuts() })
}

/// Saves a config from Settings, unless it was read before another save. Returns the new revision
/// and what the change needs done, or None when the save was refused.
pub(crate) fn save_from_settings(
    state: &AppState,
    mut config: Config,
    revision: Option<u64>,
) -> CmdResult<Option<(u64, SettingsEffects)>> {
    if config.presets.is_empty() {
        return Err("Keep at least one style.".into());
    }
    let previous = state.config();
    // A password manager is never read, so it stays on the excluded apps it was on.
    for id in previous.excluded_apps.iter().filter(|id| is_password_manager(Os::CURRENT, id)) {
        config.set_excluded(id, true);
    }
    let effects = settings_effects(&previous, &config);
    let saved = state.save_config_based_on(config, revision).map_err(|e| e.to_string())?;
    Ok(saved.map(|revision| (revision, effects)))
}

/// What a settings change needs done besides saving it.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct SettingsEffects {
    /// Show the AI in use in the menu-bar menu again.
    pub show_ai_status: bool,
    /// Rebuild the offline checker with the new word list.
    pub prepare_words: bool,
    /// Update the menu bar's "Turn off in …" item.
    pub refresh_app_toggle: bool,
    /// Register the shortcuts again.
    pub register_shortcuts: bool,
}

pub(crate) fn settings_effects(old: &Config, new: &Config) -> SettingsEffects {
    SettingsEffects {
        show_ai_status: old.active_provider != new.active_provider,
        prepare_words: old.words != new.words,
        refresh_app_toggle: old.excluded_apps != new.excluded_apps,
        register_shortcuts: old.hotkeys.review != new.hotkeys.review
            || old.hotkeys.quick_fix != new.hotkeys.quick_fix
            || old.presets.iter().map(|p| &p.shortcut).ne(new.presets.iter().map(|p| &p.shortcut)),
    }
}

/// Shortcuts the desktop left unbound because something else uses their keys, as Settings shows
/// them. Only Linux keeps them: elsewhere a refused shortcut is reported when it's saved.
fn refused_shortcuts() -> Vec<String> {
    #[cfg(target_os = "linux")]
    return platform::refused_shortcuts().into_iter().map(|(_, problem)| problem).collect();
    #[cfg(not(target_os = "linux"))]
    Vec::new()
}

#[tauri::command]
pub fn set_api_key(
    app: AppHandle,
    state: State<'_, AppState>,
    profile_id: String,
    key: Option<String>,
) -> CmdResult<()> {
    if store_api_key(&state, &secrets::CredentialStore, &profile_id, key)? {
        crate::show_ai_status(&app);
    }
    Ok(())
}

/// Saves a provider's key, or deletes it when `key` is None or blank. Returns whether that provider
/// is the AI in use, whose status the menu-bar menu then shows again.
pub(crate) fn store_api_key(
    state: &AppState,
    secrets: &dyn secrets::Secrets,
    profile_id: &str,
    key: Option<String>,
) -> CmdResult<bool> {
    state.update_provider_config(None, |config| {
        let profile = config.providers.iter().find(|p| p.id == profile_id).ok_or("Unknown provider.")?;
        let key = key.map(|key| key.trim().to_string()).filter(|key| !key.is_empty());
        if key.is_some() && !secrets::can_store_key(profile.kind) {
            return Err("This AI doesn’t use an API key.".into());
        }
        state.write_key(secrets, profile.kind, profile_id, key)?;
        Ok(config.active_provider.as_deref() == Some(profile_id))
    })
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export, rename = "Candidate"))]
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
fn matching_profile(state: &AppState, candidate: &Candidate) -> CmdResult<Option<ProviderProfile>> {
    for profile in state.config().providers.into_iter().filter(|p| p.kind == candidate.kind) {
        let matches = match &candidate.api_key {
            Some(key) => state.profile_key(&profile)?.as_ref() == Some(key),
            None => profile.base_url == candidate.base_url,
        };
        if matches {
            return Ok(Some(profile));
        }
    }
    Ok(None)
}

#[tauri::command]
pub async fn scan_accounts(state: State<'_, AppState>) -> CmdResult<Vec<CandidateView>> {
    let home = std::env::home_dir().unwrap_or_default();
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
            profile_id: matching_profile(&state, c).ok().flatten().map(|p| p.id),
        })
        .collect();
    *state.candidates.lock().unwrap() = found;
    Ok(views)
}

/// Turns a detected account into a provider profile, picks its fastest model, and makes it
/// the active provider if it passes its check (otherwise it is added, and the error returned).
#[tauri::command]
pub async fn adopt_candidate(app: AppHandle, state: State<'_, AppState>, index: usize) -> CmdResult<ProviderProfile> {
    let profile = adopt(&state, &secrets::CredentialStore, &state.engine.providers, index).await?;
    crate::show_ai_status(&app);
    Ok(profile)
}

/// `adopt_candidate` without the menu bar: adds the scanned account at `index` (or finds the
/// provider already added from it) and makes it the AI in use if it passes its check.
pub(crate) async fn adopt(
    state: &AppState,
    secrets: &dyn secrets::Secrets,
    servers: &impl Servers,
    index: usize,
) -> CmdResult<ProviderProfile> {
    let choice = state.begin_provider_choice();
    let candidate =
        state.candidates.lock().unwrap().get(index).cloned().ok_or("That result has expired. Scan again.")?;
    // Already added: switch to it rather than adding a duplicate.
    if let Some(existing) = matching_profile(state, &candidate)? {
        activate_choice(state, servers, &existing.id, choice).await?;
        return Ok(ProviderProfile { api_key: None, ..existing });
    }
    let mut profile = ProviderProfile::new(candidate.kind);
    profile.base_url = candidate.base_url.clone();
    profile.pin_official_address();
    profile.api_key = candidate.api_key.clone();
    if let Some(models) = servers.models(&profile).await {
        if !models.is_empty() {
            profile.model = preferred_model(candidate.kind, &models);
        }
    } else if !candidate.models.is_empty() {
        profile.model = preferred_model(candidate.kind, &candidate.models);
    }
    let key = profile.api_key.take();
    // Persist its ID before writing a key, so a failed save cannot leave an untracked key.
    state.update_provider_config(Some(choice), |config| {
        profile.id = unique_id(config, profile.kind);
        config.providers.push(profile.clone());
        Ok(())
    })?;
    if let Some(key) = key {
        state.update_provider_config(Some(choice), |config| {
            if !config.providers.contains(&profile) {
                return Err("This AI’s settings changed while it was being added. Try again.".into());
            }
            let result = state.write_key(secrets, profile.kind, &profile.id, Some(key));
            // On a refused save, discard the new profile only if the store confirms that
            // no key was written. Otherwise its ID makes cleanup possible.
            if result.is_err() && secrets.get(&secrets::account(profile.kind, &profile.id)) == Ok(None) {
                config.providers.retain(|saved| saved.id != profile.id);
            }
            Ok(result)
        })??;
    }
    // Added either way, but in use only if it works.
    activate_choice(state, servers, &profile.id, choice).await?;
    Ok(profile)
}

/// What the AI providers' servers say, over the network in the app ([`Providers`]) and made up in
/// tests.
pub(crate) trait Servers: Sync {
    /// The models the server lists, or None when it couldn't be asked.
    fn models(&self, profile: &ProviderProfile) -> impl Future<Output = Option<Vec<String>>> + Send;
    /// Whether the AI can answer reviews, with the models it offers (see `Providers::check`).
    fn check(&self, profile: &ProviderProfile) -> impl Future<Output = Result<Vec<String>, CheckProblem>> + Send;
}

impl Servers for Providers {
    async fn models(&self, profile: &ProviderProfile) -> Option<Vec<String>> {
        self.list_models(profile).await.ok()
    }

    async fn check(&self, profile: &ProviderProfile) -> Result<Vec<String>, CheckProblem> {
        Providers::check(self, profile).await
    }
}

/// Whether an AI is ready to review: its key is saved and its server answers (a model-list
/// request), or for the Claude subscription, the `claude` program runs.
#[derive(Serialize, Clone, Debug)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ProviderCheck {
    ready: bool,
    /// What's wrong, in words the user can act on.
    problem: Option<String>,
    /// For a problem the settings UI labels or words itself, and may offer to fix, its code.
    #[cfg_attr(test, ts(optional = nullable))]
    code: Option<CheckCode>,
    models: Vec<String>,
}

impl ProviderCheck {
    pub fn problem(&self) -> Option<&str> {
        if self.ready { None } else { Some(self.problem.as_deref().unwrap_or("not working")) }
    }
}

pub async fn check(state: &AppState, profile_id: &str) -> ProviderCheck {
    check_with(state, &state.engine.providers, profile_id).await
}

async fn check_with(state: &AppState, servers: &impl Servers, profile_id: &str) -> ProviderCheck {
    match state.profile(profile_id) {
        Ok(profile) => check_profile(servers, profile.as_ref()).await,
        Err(error) => ProviderCheck { ready: false, problem: Some(error), code: None, models: Vec::new() },
    }
}

/// Checks `profile` as given, saved or not.
async fn check_profile(servers: &impl Servers, profile: Option<&ProviderProfile>) -> ProviderCheck {
    let Some(profile) = profile else {
        return ProviderCheck {
            ready: false,
            problem: Some("This AI was removed.".into()),
            code: None,
            models: Vec::new(),
        };
    };
    match servers.check(profile).await {
        // The saved model must be one the server offers; activation picks one if it isn't.
        Ok(models) if !models.contains(&profile.model) => ProviderCheck {
            ready: false,
            problem: Some(if profile.model.is_empty() {
                "Choose a model".into()
            } else {
                format!("Choose a model: {} isn’t available", profile.model)
            }),
            code: Some(CheckCode::ChooseModel),
            models,
        },
        Ok(models) => ProviderCheck { ready: true, problem: None, code: None, models },
        Err(problem) => ProviderCheck {
            ready: false,
            problem: Some(problem.to_string()),
            code: problem.check_code(),
            models: Vec::new(),
        },
    }
}

/// Starts Claude Code's own sign-in for a Claude subscription profile, in a console window the user
/// sees (Windows). Settings then checks again every few seconds until it's signed in.
#[tauri::command]
pub fn claude_sign_in(state: State<'_, AppState>, profile_id: String) -> CmdResult<()> {
    let profile = state.profile(&profile_id)?.ok_or("This AI was removed.")?;
    if profile.kind != ProviderKind::ClaudeCli {
        return Err("Only Claude Code signs in this way.".into());
    }
    #[cfg(windows)]
    return supereditr_core::provider::start_claude_sign_in(&profile.base_url)
        .map_err(|e| format!("Couldn’t start Claude Code’s sign-in: {e}"));
    #[cfg(not(windows))]
    Err("Sign in to Claude Code in a terminal: run `claude auth login`.".into())
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
    let result = activate(&state, &state.engine.providers, profile_id).await?;
    crate::show_ai_status(&app);
    Ok(result)
}

/// `activate_provider` without the menu bar.
pub(crate) async fn activate(state: &AppState, servers: &impl Servers, profile_id: String) -> CmdResult<ProviderCheck> {
    let choice = state.begin_provider_choice();
    activate_choice(state, servers, &profile_id, choice).await
}

async fn activate_choice(
    state: &AppState,
    servers: &impl Servers,
    profile_id: &str,
    choice: u64,
) -> CmdResult<ProviderCheck> {
    // The profile as it is now: both checks use it, and the activation is saved only if it is
    // still what the settings hold.
    let checked = state.profile(profile_id)?;
    let mut result = check_profile(servers, checked.as_ref()).await;
    // Reachable, but its saved model isn't offered: switch to the fastest one it has. The switch is
    // checked on a copy and saved only with the activation, so a failed one changes nothing.
    let mut switched = None;
    if !result.ready
        && !result.models.is_empty()
        && let Some(mut candidate) = checked.clone()
    {
        candidate.model = preferred_model(candidate.kind, &result.models);
        result = check_profile(servers, Some(&candidate)).await;
        switched = Some(candidate.model);
    }
    if !result.ready {
        return Err(result.problem.unwrap_or_else(|| "This AI isn’t working yet.".into()));
    }
    commit_activation(state, &checked.ok_or("This AI was removed.")?, switched, choice)?;
    Ok(result)
}

fn commit_activation(state: &AppState, checked: &ProviderProfile, model: Option<String>, choice: u64) -> CmdResult<()> {
    state.update_provider_config(Some(choice), |config| {
        let saved = config.providers.iter_mut().find(|p| p.id == checked.id).ok_or("This AI was removed.")?;
        let mut current = saved.clone();
        current.api_key = state.profile_key(saved)?;
        if &current != checked {
            return Err("This AI’s settings changed while it was being checked. Try again.".into());
        }
        if let Some(model) = model {
            saved.model = model;
        }
        config.active_provider = Some(checked.id.clone());
        Ok(())
    })
}

#[tauri::command]
pub fn use_builtin_provider(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    use_builtin(&state)?;
    crate::show_ai_status(&app);
    Ok(())
}

fn use_builtin(state: &AppState) -> CmdResult<()> {
    let choice = state.begin_provider_choice();
    state.update_provider_config(Some(choice), |config| {
        config.active_provider = None;
        Ok(())
    })
}

fn unique_id(config: &Config, kind: ProviderKind) -> String {
    let base = ProviderProfile::new(kind).id;
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
    state.update_provider_config(None, |config| {
        let mut profile = ProviderProfile::new(kind);
        profile.id = unique_id(config, kind);
        config.providers.push(profile.clone());
        Ok(profile)
    })
}

#[tauri::command]
pub fn remove_provider(app: AppHandle, state: State<'_, AppState>, profile_id: String) -> CmdResult<()> {
    let result = remove(&state, &secrets::CredentialStore, &profile_id);
    crate::show_ai_status(&app);
    result
}

fn remove(state: &AppState, secrets: &dyn secrets::Secrets, profile_id: &str) -> CmdResult<()> {
    let choice = state.begin_provider_choice();
    state.update_provider_config(Some(choice), |config| {
        if let Some(profile) = config.providers.iter().find(|p| p.id == profile_id)
            && secrets::can_store_key(profile.kind)
        {
            state.write_key(secrets, profile.kind, profile_id, None)?;
        }
        config.providers.retain(|p| p.id != profile_id);
        if config.active_provider.as_deref() == Some(profile_id) {
            config.active_provider = None;
        }
        Ok(())
    })
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    first_token_ms: Option<u64>,
    total_ms: u64,
    output: String,
}

/// Sends a tiny proofreading request and reports latency.
#[tauri::command]
pub async fn test_provider(state: State<'_, AppState>, profile_id: String) -> CmdResult<TestResult> {
    let profile = state.profile(&profile_id)?.ok_or("Unknown provider.")?;
    let providers = &state.engine.providers;
    providers.warm(&profile).await;
    let request = ChatRequest {
        system: "Fix the spelling and grammar of the text in <text> tags. Reply with the corrected text only.".into(),
        user: "<text>This are a quick tset.</text>".into(),
        max_tokens: 64,
        guided: false,
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

#[derive(Serialize, Clone)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct OllamaPullProgress {
    request_id: String,
    progress: PullProgress,
}

#[tauri::command]
pub async fn pull_ollama_model(
    app: AppHandle,
    state: State<'_, AppState>,
    request_id: String,
    profile_id: String,
    base_url: String,
    model: String,
) -> CmdResult<ProviderCheck> {
    let choice = state.begin_provider_choice();
    let checked = state.profile(&profile_id)?.ok_or("This AI was removed.")?;
    if checked.kind != ProviderKind::Ollama || checked.base_url != base_url {
        return Err("This Ollama server’s settings changed. Try again.".into());
    }
    state
        .engine
        .providers
        .ollama_pull(&base_url, &model, &mut |progress| {
            let _ = app.emit_to(
                "settings",
                "settings:pull",
                OllamaPullProgress { request_id: request_id.clone(), progress },
            );
        })
        .await
        .map_err(|e| e.to_string())?;
    let result = activate_pulled(&state, &state.engine.providers, &checked, model, choice).await?;
    crate::show_ai_status(&app);
    Ok(result)
}

async fn activate_pulled(
    state: &AppState,
    servers: &impl Servers,
    checked: &ProviderProfile,
    model: String,
    choice: u64,
) -> CmdResult<ProviderCheck> {
    let mut requested = checked.clone();
    requested.model = model.clone();
    let result = check_profile(servers, Some(&requested)).await;
    if !result.ready {
        return Err(result.problem.unwrap_or_else(|| "This AI isn’t working yet.".into()));
    }
    commit_activation(state, checked, Some(model), choice)?;
    Ok(result)
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
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
    shortcut_problem(
        &state.config(),
        &accelerator,
        &slot,
        crate::shortcut_check::user_hotkeys,
        || desktop_use(&accelerator),
        || hotkeys::taken_by_another_app(&app, &accelerator),
    )
}

/// What the Linux desktop uses a shortcut for, if anything.
fn desktop_use(accelerator: &str) -> Option<String> {
    #[cfg(target_os = "linux")]
    return platform::shortcut_taken(accelerator);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = accelerator;
        None
    }
}

/// `check_shortcut`, with what the system knows asked through the last three arguments:
/// `user_hotkeys`, the shortcuts the user set in macOS (not asked on Windows); `desktop_use`, what
/// the Linux desktop uses it for (asked only on Linux); and `taken_by_another_app`. Each is asked
/// only when the checks before it found nothing.
pub(crate) fn shortcut_problem(
    config: &Config,
    accelerator: &str,
    slot: &str,
    user_hotkeys: impl FnOnce() -> Vec<crate::shortcut_check::UserHotkey>,
    desktop_use: impl FnOnce() -> Option<String>,
    taken_by_another_app: impl FnOnce() -> bool,
) -> Option<String> {
    let others = hotkeys::others(config, slot);
    #[cfg(windows)]
    let problem = {
        let _ = user_hotkeys;
        crate::keys::windows_conflict(accelerator, &others)
    };
    #[cfg(not(windows))]
    let problem = crate::shortcut_check::conflict(accelerator, &others, &user_hotkeys());
    if problem.is_some() {
        return problem;
    }
    #[cfg(target_os = "linux")]
    if let Some(what) = desktop_use() {
        return Some(format!("Your desktop already uses this for “{what}”."));
    }
    #[cfg(not(target_os = "linux"))]
    let _ = desktop_use;
    let taken = if cfg!(windows) {
        "Windows or another app uses this shortcut."
    } else {
        "Another app already uses this shortcut."
    };
    taken_by_another_app().then(|| taken.into())
}

/// The built-in styles as first installed, for resetting the Styles page.
#[tauri::command]
pub fn default_presets() -> Vec<supereditr_core::prompts::Preset> {
    supereditr_core::prompts::builtin_presets()
}

/// Apple Intelligence, the AI SuperEditr uses unless another is chosen.
#[tauri::command]
pub async fn builtin_ai(state: State<'_, AppState>) -> CmdResult<BuiltinAi> {
    // Apple Intelligence exists only on a Mac; elsewhere the setup screens never mention it.
    if cfg!(not(target_os = "macos")) {
        return Ok(BuiltinAi { supported: false, available: false, message: None });
    }
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
    #[cfg(target_os = "macos")]
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
    // The web view fills the window below its title bar, and the panel is placed in physical pixels.
    #[cfg(windows)]
    let bounds = match (window.inner_position(), window.scale_factor()) {
        (Ok(at), Ok(scale)) => platform::Rect {
            x: f64::from(at.x) + rect.x * scale,
            y: f64::from(at.y) + rect.y * scale,
            width: rect.width * scale,
            height: rect.height * scale,
        },
        _ => rect,
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let bounds = {
        let _ = window;
        rect
    };
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
    #[cfg(windows)]
    fit_on_screen(&window, Some(height));
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (window, height);
}

/// Sizes Settings to `height` logical pixels of page (its current height when None), as far as the
/// screen above the taskbar allows with the title bar, and moves it up if its bottom would be
/// hidden. Settings opens at 760 high, taller than a small laptop's screen at 150%.
#[cfg(windows)]
pub fn fit_on_screen(window: &tauri::WebviewWindow, height: Option<f64>) {
    let (Ok(scale), Ok(inner), Ok(outer), Ok(position), Ok(Some(monitor))) = (
        window.scale_factor(),
        window.inner_size(),
        window.outer_size(),
        window.outer_position(),
        window.current_monitor(),
    ) else {
        return;
    };
    let work = monitor.work_area();
    let frame = outer.height.saturating_sub(inner.height);
    let available = f64::from(work.size.height.saturating_sub(frame)) / scale;
    let wanted = height.unwrap_or(f64::from(inner.height) / scale);
    let page = (wanted.min(available).max(480.0) * scale).round() as u32;
    if inner.height.abs_diff(page) > 1 {
        let _ = window.set_size(tauri::PhysicalSize::new(inner.width, page));
    }
    let work_bottom = work.position.y + work.size.height as i32;
    let top = position.y.min(work_bottom - (page + frame) as i32).max(work.position.y);
    if top != position.y {
        let _ = window.set_position(tauri::PhysicalPosition::new(position.x, top));
    }
}

/// Turns SuperEditr's shortcuts off while Settings records one (see `hotkeys::pause`).
#[tauri::command]
pub fn pause_shortcuts(app: AppHandle, paused: bool) {
    hotkeys::pause(&app, paused);
}

/// Whether macOS opens SuperEditr when the user logs in, including after a restart.
#[tauri::command]
pub fn launch_at_login(app: AppHandle) -> bool {
    crate::login_item::is_enabled(&app).unwrap_or(false)
}

/// Turns opening at login on or off and returns the state macOS now reports.
#[tauri::command]
pub fn set_launch_at_login(app: AppHandle, enabled: bool) -> CmdResult<bool> {
    let result = if enabled { crate::login_item::enable(&app) } else { crate::login_item::disable(&app) };
    result.map_err(|e| format!("Couldn't change opening at login: {e}"))?;
    #[cfg(target_os = "linux")]
    platform::setup::reload_autostart();
    crate::login_item::is_enabled(&app)
}

/// The Linux setup page's report; `None` on other systems.
#[cfg(target_os = "linux")]
#[tauri::command]
pub async fn linux_setup() -> Option<platform::SetupReport> {
    Some(platform::setup_report().await)
}

#[cfg(not(target_os = "linux"))]
#[tauri::command]
pub async fn linux_setup() -> Option<()> {
    None
}

/// Set up Omarchy: adds the flag lines and turns on opening at login. Returns what it did.
#[tauri::command]
pub fn set_up_omarchy(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Vec<String>> {
    #[cfg(target_os = "linux")]
    {
        use tauri_plugin_autostart::ManagerExt;
        let config_home = platform::setup::config_home().ok_or("HOME isn’t set")?;
        let mut done = platform::setup::add_flags(&config_home).map_err(|e| format!("Couldn’t add the flag: {e}"))?;
        if !platform::setup::autostart_check(&config_home).ok {
            app.autolaunch().enable().map_err(|e| format!("Couldn’t turn on opening at login: {e}"))?;
            platform::setup::reload_autostart();
            done.push("Turned on opening at login".into());
        }
        let mut config = state.config();
        if !config.offered_launch_at_login {
            config.offered_launch_at_login = true;
            state.save_config(config).map_err(|e| e.to_string())?;
        }
        Ok(done)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, state);
        Err("Set up Omarchy is for Linux.".into())
    }
}

/// Takes out what Set up Omarchy added, as `supereditr uninstall-omarchy` does. Returns what it did.
#[tauri::command]
pub fn remove_omarchy_setup() -> CmdResult<Vec<String>> {
    #[cfg(target_os = "linux")]
    {
        let config_home = platform::setup::config_home().ok_or("HOME isn’t set")?;
        let mut done = platform::setup::remove_flags(&config_home).map_err(|e| e.to_string())?;
        done.extend(platform::setup::remove_autostart(&config_home).map_err(|e| e.to_string())?);
        platform::setup::reload_autostart();
        Ok(done)
    }
    #[cfg(not(target_os = "linux"))]
    Err("Set up Omarchy is for Linux.".into())
}

#[tauri::command]
pub fn accessibility_status(prompt: bool) -> bool {
    platform::accessibility_trusted(prompt)
}

/// Names and icons for the installed apps among `bundle_ids`, keyed by bundle ID; an app that
/// isn't installed is left out. Synchronous, so it runs on the main thread, where AppKit draws.
#[tauri::command]
pub fn app_info(bundle_ids: Vec<String>) -> HashMap<String, apps::AppInfo> {
    bundle_ids.into_iter().filter_map(|id| Some((id.clone(), apps::find(&id)?))).collect()
}

/// The password managers among `bundle_ids`: Settings → Privacy offers no Remove for them, since
/// nothing is read from one whatever the excluded apps say.
#[tauri::command]
pub fn password_managers(bundle_ids: Vec<String>) -> Vec<String> {
    bundle_ids.into_iter().filter(|id| is_password_manager(Os::CURRENT, id)).collect()
}

/// Opens a panel on /Applications as a sheet on the calling window and returns the apps the user
/// chose there.
#[tauri::command]
pub async fn pick_apps(window: tauri::WebviewWindow) -> apps::Picked {
    let (sender, chosen) = tokio::sync::oneshot::channel();
    let target = window.clone();
    let _ = window.run_on_main_thread(move || {
        apps::pick(&target, move |picked| {
            let _ = sender.send(picked);
        })
    });
    chosen.await.unwrap_or_default()
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

#[tauri::command]
pub fn update_status(app: AppHandle) -> crate::updater::Status {
    crate::updater::status(&app)
}

/// The update window's main button: Restart Now or Try Again.
#[tauri::command]
pub fn update_now(app: AppHandle) {
    crate::updater::on_menu(&app);
}

#[tauri::command]
pub fn close_update_window(window: tauri::WebviewWindow) {
    let _ = window.close();
}

/// Synchronous so it runs on the main thread while the mouse is still down.
#[tauri::command]
pub fn drag_app(window: tauri::WebviewWindow) -> CmdResult<()> {
    crate::permission_guide::drag_app(&window)
}
