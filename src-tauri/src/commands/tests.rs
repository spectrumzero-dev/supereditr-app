use super::*;
use crate::secrets::FakeSecrets;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// An `AppState` with its settings in a folder of its own, deleted afterwards.
struct TestState {
    state: AppState,
    dir: PathBuf,
}

impl std::ops::Deref for TestState {
    type Target = AppState;
    fn deref(&self) -> &AppState {
        &self.state
    }
}

impl Drop for TestState {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn test_state() -> TestState {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("supereditr-commands-{}-{n}", std::process::id()));
    TestState { state: AppState::new(dir.join("settings.json")), dir }
}

/// A key no provider would accept, built from pieces.
fn fake_key(owner: &str) -> String {
    ["test", "key", owner].join("-")
}

/// Adds a provider of `kind` to the settings and returns its ID.
fn add(state: &AppState, kind: ProviderKind, model: &str) -> String {
    let mut profile = ProviderProfile::new(kind);
    profile.id = unique_id(state, kind);
    profile.model = model.into();
    let mut config = state.config();
    config.providers.push(profile.clone());
    state.save_config(config).unwrap();
    profile.id
}

/// Servers that answer without the network: each lists `offered` and passes its check when its key
/// is present, or none answers at all when `reachable` is false.
struct FakeServers {
    offered: Vec<String>,
    reachable: bool,
    /// The profiles checked, by ID, in order.
    checked: Mutex<Vec<String>>,
}

impl FakeServers {
    fn offering(models: &[&str]) -> Self {
        Self { offered: models.iter().map(|m| m.to_string()).collect(), reachable: true, checked: Mutex::default() }
    }

    fn unreachable() -> Self {
        Self { reachable: false, ..Self::offering(&[]) }
    }
}

impl Servers for FakeServers {
    async fn models(&self, _profile: &ProviderProfile) -> Option<Vec<String>> {
        self.reachable.then(|| self.offered.clone())
    }

    async fn check(&self, profile: &ProviderProfile) -> Result<Vec<String>, CheckProblem> {
        self.checked.lock().unwrap().push(profile.id.clone());
        if profile.kind.needs_key() && profile.api_key.is_none() {
            return Err("Add an API key".into());
        }
        if !self.reachable {
            return Err("Can’t connect: no answer in 8 seconds".into());
        }
        Ok(self.offered.clone())
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    tauri::async_runtime::block_on(future)
}

// --- save_settings -------------------------------------------------------------------------

#[test]
fn an_unchanged_config_needs_nothing_done() {
    let config = Config::default();
    assert_eq!(settings_effects(&config, &config.clone()), SettingsEffects::default());
}

#[test]
fn choosing_another_ai_shows_its_status() {
    let old = Config::default();
    let mut new = old.clone();
    new.active_provider = Some("anthropic".into());
    assert_eq!(settings_effects(&old, &new), SettingsEffects { show_ai_status: true, ..Default::default() });
}

#[test]
fn switching_back_to_the_built_in_ai_shows_its_status() {
    let old = Config { active_provider: Some("anthropic".into()), ..Default::default() };
    let new = Config { active_provider: None, ..old.clone() };
    assert!(settings_effects(&old, &new).show_ai_status);
}

#[test]
fn a_changed_word_list_rebuilds_the_offline_checker() {
    let old = Config::default();
    let mut new = old.clone();
    new.words.push("Brynlee".into());
    assert_eq!(settings_effects(&old, &new), SettingsEffects { prepare_words: true, ..Default::default() });
}

#[test]
fn a_changed_list_of_excluded_apps_updates_the_menu_bar_item() {
    let old = Config::default();
    let mut new = old.clone();
    new.set_excluded("com.quillpoint.notes", true);
    assert_eq!(settings_effects(&old, &new), SettingsEffects { refresh_app_toggle: true, ..Default::default() });
}

#[test]
fn a_changed_shortcut_registers_the_shortcuts_again() {
    let old = Config::default();
    let only_shortcuts = SettingsEffects { register_shortcuts: true, ..Default::default() };

    let mut review = old.clone();
    review.hotkeys.review = "Control+Shift+F7".into();
    assert_eq!(settings_effects(&old, &review), only_shortcuts, "the review shortcut");

    let mut quick_fix = old.clone();
    quick_fix.hotkeys.quick_fix = "Control+Shift+F7".into();
    assert_eq!(settings_effects(&old, &quick_fix), only_shortcuts, "the quick fix shortcut");

    let mut style = old.clone();
    style.presets[0].shortcut = Some("Control+Shift+F7".into());
    assert_eq!(settings_effects(&old, &style), only_shortcuts, "a style's shortcut");

    let mut removed = old.clone();
    removed.presets.pop();
    assert!(settings_effects(&old, &removed).register_shortcuts, "a style removed");
}

#[test]
fn other_changes_to_a_style_leave_the_shortcuts_alone() {
    let old = Config::default();
    let mut new = old.clone();
    new.presets[0].name = "Kyndal’s proofread".into();
    new.style_notes = "Short sentences.".into();
    assert_eq!(settings_effects(&old, &new), SettingsEffects::default());
}

#[test]
fn a_save_reports_what_changed_since_the_settings_in_use() {
    let state = test_state();
    let (mut config, revision) = state.config_at_revision();
    config.words.push("Rhyan".into());
    let (saved_at, effects) = save_from_settings(&state, config, Some(revision)).unwrap().unwrap();
    assert_eq!(saved_at, revision + 1);
    assert_eq!(effects, SettingsEffects { prepare_words: true, ..Default::default() });
    assert_eq!(state.config().words, ["Rhyan"]);
}

#[test]
fn a_save_from_settings_read_before_another_save_is_refused() {
    let state = test_state();
    let (mut settings_copy, read_at) = state.config_at_revision();
    // The panel's "Always keep" saves while Settings has its copy.
    let mut panel = state.config();
    panel.words.push("Kyndal".into());
    state.save_config(panel).unwrap();
    settings_copy.style_notes = "Short sentences.".into();
    assert_eq!(save_from_settings(&state, settings_copy, Some(read_at)).unwrap(), None);
    assert_eq!(state.config().words, ["Kyndal"]);
    assert_eq!(state.config().style_notes, "");
}

#[test]
fn a_save_without_a_revision_is_never_refused() {
    let state = test_state();
    let mut other = state.config();
    other.words.push("Kyndal".into());
    state.save_config(other).unwrap();
    let mut config = state.config();
    config.style_notes = "Short sentences.".into();
    assert!(save_from_settings(&state, config, None).unwrap().is_some());
}

#[test]
fn a_config_without_styles_is_not_saved() {
    let state = test_state();
    let mut config = state.config();
    config.presets.clear();
    assert_eq!(save_from_settings(&state, config, None).unwrap_err(), "Keep at least one style.");
    assert!(!state.config().presets.is_empty());
}

// --- set_api_key ---------------------------------------------------------------------------

#[test]
fn a_new_key_is_saved_trimmed_and_used_at_once() {
    let state = test_state();
    let secrets = FakeSecrets::default();
    let id = add(&state, ProviderKind::Anthropic, "claude-sonnet-5-5");
    let key = fake_key("rhyan");
    assert!(!store_api_key(&state, &secrets, &id, Some(format!("  {key}\n"))).unwrap(), "not the AI in use");
    assert_eq!(secrets.key(ProviderKind::Anthropic, &id), Some(key.clone()));
    assert_eq!(state.key_for(ProviderKind::Anthropic, &id), Some(key));
}

#[test]
fn a_blank_or_missing_key_deletes_the_saved_one() {
    let state = test_state();
    let secrets = FakeSecrets::default();
    let id = add(&state, ProviderKind::Gemini, "gemini-flash");
    for cleared in [None, Some("   ".to_string())] {
        store_api_key(&state, &secrets, &id, Some(fake_key("brynlee"))).unwrap();
        store_api_key(&state, &secrets, &id, cleared.clone()).unwrap();
        assert_eq!(secrets.key(ProviderKind::Gemini, &id), None, "{cleared:?}");
        assert_eq!(state.key_for(ProviderKind::Gemini, &id), None, "{cleared:?}");
    }
}

#[test]
fn a_key_for_the_ai_in_use_shows_its_status_again() {
    let state = test_state();
    let id = add(&state, ProviderKind::OpenAi, "gpt-5-mini");
    let mut config = state.config();
    config.active_provider = Some(id.clone());
    state.save_config(config).unwrap();
    assert!(store_api_key(&state, &FakeSecrets::default(), &id, Some(fake_key("kyndal"))).unwrap());
}

#[test]
fn a_key_for_an_unknown_provider_is_refused() {
    let state = test_state();
    let secrets = FakeSecrets::default();
    assert_eq!(
        store_api_key(&state, &secrets, "quillpoint", Some(fake_key("rhyan"))).unwrap_err(),
        "Unknown provider."
    );
    assert!(secrets.saved.lock().unwrap().is_empty());
}

#[test]
fn a_key_the_credential_store_refuses_is_not_used() {
    let state = test_state();
    let secrets = FakeSecrets { refuse: true, ..Default::default() };
    let id = add(&state, ProviderKind::Anthropic, "claude-sonnet-5-5");
    assert!(store_api_key(&state, &secrets, &id, Some(fake_key("rhyan"))).is_err());
    assert_eq!(state.key_for(ProviderKind::Anthropic, &id), None);
}

// --- activate_provider ---------------------------------------------------------------------

#[test]
fn a_provider_without_its_key_is_not_made_the_ai_in_use() {
    let state = test_state();
    let id = add(&state, ProviderKind::Anthropic, "claude-sonnet-5-5");
    let servers = FakeServers::offering(&["claude-sonnet-5-5"]);
    assert_eq!(block_on(activate(&state, &servers, id)).unwrap_err(), "Add an API key");
    assert_eq!(state.config().active_provider, None, "reviews keep the built-in AI");
}

#[test]
fn a_provider_with_its_key_becomes_the_ai_in_use() {
    let state = test_state();
    let id = add(&state, ProviderKind::Anthropic, "claude-sonnet-5-5");
    state.set_cached_key(ProviderKind::Anthropic, &id, Some(fake_key("brynlee")));
    let servers = FakeServers::offering(&["claude-haiku-5-5", "claude-sonnet-5-5"]);
    let result = block_on(activate(&state, &servers, id.clone())).unwrap();
    assert!(result.ready);
    assert_eq!(result.models, ["claude-haiku-5-5", "claude-sonnet-5-5"]);
    assert_eq!(state.config().active_provider, Some(id));
}

#[test]
fn a_provider_whose_model_is_gone_switches_to_one_its_server_offers() {
    let state = test_state();
    let id = add(&state, ProviderKind::Ollama, "retired-model");
    let servers = FakeServers::offering(&["mistral", "qwen3:4b"]);
    assert!(block_on(activate(&state, &servers, id.clone())).unwrap().ready);
    assert_eq!(state.profile(&id).unwrap().model, "qwen3:4b");
    assert_eq!(state.config().active_provider, Some(id.clone()));
    assert_eq!(*servers.checked.lock().unwrap(), [id.clone(), id], "checked again with the new model");
}

#[test]
fn a_provider_that_doesnt_answer_keeps_the_ai_in_use() {
    let state = test_state();
    let working = add(&state, ProviderKind::Ollama, "mistral");
    let mut config = state.config();
    config.active_provider = Some(working.clone());
    state.save_config(config).unwrap();
    let broken = add(&state, ProviderKind::Ollama, "mistral");
    let error = block_on(activate(&state, &FakeServers::unreachable(), broken)).unwrap_err();
    assert_eq!(error, "Can’t connect: no answer in 8 seconds");
    assert_eq!(state.config().active_provider, Some(working));
}

#[test]
fn a_removed_provider_is_not_made_the_ai_in_use() {
    let state = test_state();
    let servers = FakeServers::offering(&["mistral"]);
    assert_eq!(block_on(activate(&state, &servers, "ollama".into())).unwrap_err(), "This AI was removed.");
    assert_eq!(state.config().active_provider, None);
}

// --- adopt_candidate -----------------------------------------------------------------------

fn candidate(kind: ProviderKind, api_key: Option<String>, models: &[&str]) -> Candidate {
    Candidate {
        kind,
        source: "found by a test".into(),
        api_key,
        key_hint: None,
        base_url: kind.default_base_url().into(),
        models: models.iter().map(|m| m.to_string()).collect(),
        note: None,
        ready: true,
    }
}

#[test]
fn an_account_with_a_key_is_added_with_its_key_and_used() {
    let state = test_state();
    let secrets = FakeSecrets::default();
    let key = fake_key("rhyan");
    *state.candidates.lock().unwrap() = vec![candidate(ProviderKind::Anthropic, Some(key.clone()), &[])];
    let servers = FakeServers::offering(&["claude-haiku-5-5", "claude-sonnet-5-5"]);
    let profile = block_on(adopt(&state, &secrets, &servers, 0)).unwrap();
    assert_eq!(profile.api_key, None, "the key never goes back to Settings");
    assert_eq!(profile.model, "claude-sonnet-5-5", "the preferred model the server lists");
    assert_eq!(secrets.key(ProviderKind::Anthropic, &profile.id), Some(key));
    let config = state.config();
    assert_eq!(config.providers.len(), 1);
    assert_eq!(config.providers[0].api_key, None, "keys stay out of the settings file");
    assert_eq!(config.active_provider, Some(profile.id));
}

#[test]
fn an_account_already_added_is_switched_to_instead_of_added_again() {
    let state = test_state();
    let secrets = FakeSecrets::default();
    let key = fake_key("brynlee");
    let id = add(&state, ProviderKind::Anthropic, "claude-sonnet-5-5");
    state.set_cached_key(ProviderKind::Anthropic, &id, Some(key.clone()));
    *state.candidates.lock().unwrap() = vec![candidate(ProviderKind::Anthropic, Some(key), &[])];
    let servers = FakeServers::offering(&["claude-sonnet-5-5"]);
    let profile = block_on(adopt(&state, &secrets, &servers, 0)).unwrap();
    assert_eq!(profile.id, id);
    assert_eq!(profile.api_key, None);
    assert_eq!(state.config().providers.len(), 1);
    assert_eq!(state.config().active_provider, Some(id));
    assert!(secrets.saved.lock().unwrap().is_empty(), "the key was already saved");
}

#[test]
fn a_second_account_of_the_same_kind_gets_its_own_id() {
    let state = test_state();
    let first = add(&state, ProviderKind::Anthropic, "claude-sonnet-5-5");
    state.set_cached_key(ProviderKind::Anthropic, &first, Some(fake_key("rhyan")));
    *state.candidates.lock().unwrap() = vec![candidate(ProviderKind::Anthropic, Some(fake_key("kyndal")), &[])];
    let servers = FakeServers::offering(&["claude-sonnet-5-5"]);
    let profile = block_on(adopt(&state, &FakeSecrets::default(), &servers, 0)).unwrap();
    assert_eq!(profile.id, format!("{first}-2"));
    assert_eq!(state.config().providers.len(), 2);
}

#[test]
fn an_account_that_fails_its_check_is_added_but_not_used() {
    let state = test_state();
    *state.candidates.lock().unwrap() = vec![candidate(ProviderKind::Ollama, None, &["mistral", "qwen3:4b"])];
    let error = block_on(adopt(&state, &FakeSecrets::default(), &FakeServers::unreachable(), 0)).unwrap_err();
    assert_eq!(error, "Can’t connect: no answer in 8 seconds");
    let config = state.config();
    assert_eq!(config.providers.len(), 1);
    assert_eq!(config.providers[0].model, "qwen3:4b", "the preferred model the scan found");
    assert_eq!(config.active_provider, None);
}

#[test]
fn an_expired_scan_result_asks_for_a_new_scan() {
    let state = test_state();
    let servers = FakeServers::offering(&[]);
    let error = block_on(adopt(&state, &FakeSecrets::default(), &servers, 3)).unwrap_err();
    assert_eq!(error, "That result has expired. Scan again.");
    assert!(servers.checked.lock().unwrap().is_empty());
}

// --- check_shortcut ------------------------------------------------------------------------

/// `shortcut_problem` with nothing else on the system using any shortcut.
fn problem(config: &Config, accelerator: &str, slot: &str) -> Option<String> {
    shortcut_problem(config, accelerator, slot, Vec::new, || None, || false)
}

#[test]
fn a_free_shortcut_has_no_problem() {
    assert_eq!(problem(&Config::default(), "Control+Alt+F8", "review"), None);
}

#[test]
fn a_shortcut_supereditr_uses_elsewhere_is_refused() {
    let mut config = Config::default();
    config.hotkeys.quick_fix = "Control+Alt+F8".into();
    assert_eq!(
        problem(&config, "Control+Alt+F8", "review").as_deref(),
        Some("SuperEditr already uses this for Quick fix.")
    );
}

#[test]
fn a_slot_may_keep_its_own_shortcut() {
    let mut config = Config::default();
    config.hotkeys.review = "Control+Alt+F8".into();
    assert_eq!(problem(&config, "Control+Alt+F8", "review"), None);
}

#[test]
fn a_shortcut_another_app_uses_is_refused() {
    let expected = if cfg!(windows) {
        "Windows or another app uses this shortcut."
    } else {
        "Another app already uses this shortcut."
    };
    let found = shortcut_problem(&Config::default(), "Control+Alt+F8", "review", Vec::new, || None, || true);
    assert_eq!(found.as_deref(), Some(expected));
}

#[test]
fn a_refused_shortcut_asks_the_system_nothing_more() {
    let mut config = Config::default();
    config.hotkeys.quick_fix = "Control+Alt+F8".into();
    let found = shortcut_problem(
        &config,
        "Control+Alt+F8",
        "review",
        Vec::new,
        || panic!("the desktop was asked"),
        || panic!("the other apps were asked"),
    );
    assert!(found.is_some());
}

#[cfg(target_os = "linux")]
#[test]
fn a_shortcut_the_desktop_uses_is_refused() {
    let found = shortcut_problem(
        &Config::default(),
        "Control+Alt+F8",
        "review",
        Vec::new,
        || Some("Lock screen".into()),
        || panic!("the other apps were asked"),
    );
    assert_eq!(found.as_deref(), Some("Your desktop already uses this for “Lock screen”."));
}
