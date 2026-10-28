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
}

pub struct Session {
    pub id: u64,
    pub capture: Capture,
    pub key_window: bool,
    pub anchor: Option<Rect>,
    pub height: f64,
    pub task: Option<tauri::async_runtime::JoinHandle<()>>,
}

impl Session {
    pub fn cancel(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl AppState {
    pub fn new(config_path: PathBuf) -> Self {
        let config = Config::load(&config_path);
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

    pub fn save_config(&self, config: Config) -> std::io::Result<()> {
        config.save(&self.config_path)?;
        *self.config.write().unwrap() = config;
        Ok(())
    }
}
