use crate::platform::{Capture, Rect};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::Instant;
use supereditr_core::detect::Candidate;
use supereditr_core::provider::ProviderProfile;
use supereditr_core::{Config, Engine};

pub struct AppState {
    pub config: RwLock<Config>,
    pub config_path: PathBuf,
    pub engine: Engine,
    pub session: Mutex<Option<Session>>,
    pub next_id: AtomicU64,
    /// Results of the last account scan, keys included, until the user picks one.
    pub candidates: Mutex<Vec<Candidate>>,
    /// API keys read from the keychain, cached so a review never waits on Keychain Services.
    keys: Mutex<HashMap<String, Option<String>>>,
    pub paused: AtomicBool,
    /// A standalone notice (no review) is on screen.
    pub notice_visible: AtomicBool,
    pub watch_generation: AtomicU64,
    /// Selection events are ignored until this moment (just after we replaced text).
    pub quiet_until: Mutex<Instant>,
    /// Counts saves of the settings, so Settings' save of a copy read before another save (the menu
    /// bar's "Turn off in …", the panel's "Always keep") can be refused instead of undoing it.
    revision: AtomicU64,
}

pub struct Session {
    pub id: u64,
    pub capture: Capture,
    /// A review the user opened (or switched to the keyboard): the panel takes the review keys.
    pub keyboard: bool,
    pub anchor: Option<Rect>,
    pub height: f64,
    pub task: Option<tauri::async_runtime::JoinHandle<()>>,
    /// The latest run is a plain proofread (no other style, no instruction), which is applied in
    /// place where the app allows it.
    pub proofread: bool,
}

impl Session {
    pub fn cancel(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

// Test builds keep the diagnostic log whatever the setting says, so a build that misbehaves on a
// tester's Mac has left a record even when it can no longer be reached to turn the log on. Only
// official release builds (`updater::ENABLED`) follow the setting.
impl AppState {
    pub fn new(config_path: PathBuf) -> Self {
        let config = Config::load(&config_path);
        crate::platform::set_trace(config.diagnostic_log || !crate::updater::ENABLED);
        let engine = Engine::new(&config.dialect);
        Self {
            config: RwLock::new(config),
            config_path,
            engine,
            session: Mutex::new(None),
            next_id: AtomicU64::new(1),
            candidates: Mutex::new(Vec::new()),
            keys: Mutex::new(HashMap::new()),
            paused: AtomicBool::new(false),
            notice_visible: AtomicBool::new(false),
            watch_generation: AtomicU64::new(0),
            quiet_until: Mutex::new(Instant::now()),
            revision: AtomicU64::new(0),
        }
    }

    pub fn config(&self) -> Config {
        self.config.read().unwrap().clone()
    }

    pub fn next_session_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn key_for(&self, profile_id: &str) -> Option<String> {
        let mut keys = self.keys.lock().unwrap();
        keys.entry(profile_id.to_string()).or_insert_with(|| crate::secrets::get(profile_id)).clone()
    }

    pub fn set_cached_key(&self, profile_id: &str, key: Option<String>) {
        self.keys.lock().unwrap().insert(profile_id.to_string(), key);
    }

    /// The active provider with its API key attached, ready for a request.
    pub fn active_profile(&self) -> Option<ProviderProfile> {
        let mut profile = self.config.read().unwrap().active_profile()?.clone();
        profile.api_key = self.key_for(&profile.id);
        Some(profile)
    }

    pub fn profile(&self, id: &str) -> Option<ProviderProfile> {
        let mut profile = self.config.read().unwrap().providers.iter().find(|p| p.id == id)?.clone();
        profile.api_key = self.key_for(&profile.id);
        Some(profile)
    }

    /// The settings, and the revision they were saved at.
    pub fn config_at_revision(&self) -> (Config, u64) {
        let config = self.config.read().unwrap();
        (config.clone(), self.revision.load(Ordering::Relaxed))
    }

    pub fn save_config(&self, config: Config) -> std::io::Result<()> {
        self.save_config_based_on(config, None).map(|_| ())
    }

    /// Saves, unless `revision` is given and the settings have been saved since then. Returns the
    /// new revision, or None when the save was refused.
    pub fn save_config_based_on(&self, config: Config, revision: Option<u64>) -> std::io::Result<Option<u64>> {
        // The revision changes only under this lock, so nothing can save between check and write.
        let mut current = self.config.write().unwrap();
        if revision.is_some_and(|r| r != self.revision.load(Ordering::Relaxed)) {
            return Ok(None);
        }
        config.save(&self.config_path)?;
        crate::platform::set_trace(config.diagnostic_log || !crate::updater::ENABLED);
        *current = config;
        Ok(Some(self.revision.fetch_add(1, Ordering::Relaxed) + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_based_on_settings_read_before_another_save_is_refused() {
        let dir = std::env::temp_dir().join(format!("supereditr-revision-{}", std::process::id()));
        let state = AppState::new(dir.join("settings.json"));
        let (settings_copy, read_at) = state.config_at_revision();
        // The menu bar turns SuperEditr off in Notes while Settings has its copy.
        let mut menu = state.config();
        menu.set_excluded("com.apple.Notes", true);
        state.save_config(menu).unwrap();
        // Settings' save of its older copy is refused, so Notes stays excluded.
        let mut edited = settings_copy;
        edited.style_notes = "Short sentences.".into();
        assert_eq!(state.save_config_based_on(edited.clone(), Some(read_at)).unwrap(), None);
        assert!(state.config().is_excluded("com.apple.Notes"));
        assert!(Config::load(&dir.join("settings.json")).is_excluded("com.apple.Notes"));
        // Based on the current revision, the same save goes through.
        let (_, now) = state.config_at_revision();
        assert_eq!(state.save_config_based_on(edited, Some(now)).unwrap(), Some(now + 1));
        assert_eq!(state.config().style_notes, "Short sentences.");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
