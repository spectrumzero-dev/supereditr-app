//! User settings, stored as JSON in the platform's app-config directory.
//! API keys are never part of this file; the app keeps them in the OS keychain.

use crate::prompts::{Preset, builtin_presets};
use crate::provider::ProviderProfile;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    /// Opens the overlay for the current selection.
    pub review: String,
    /// Applies the default preset to the selection immediately, without showing suggestions.
    pub quick_fix: String,
}

/// Apps where SuperEditr never reads text: password managers, and terminals, whose output reads
/// like text and where pasting a revision would run it as input.
const DEFAULT_EXCLUSIONS: &[&str] = &[
    "com.1password.1password",
    "com.nordsec.nordpass",
    "com.bitwarden.desktop",
    "com.apple.keychainaccess",
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "com.mitchellh.ghostty",
    "com.cmuxterm.app",
    "dev.warp.Warp-Stable",
    "net.kovidgoyal.kitty",
    "org.alacritty",
    "com.github.wez.wezterm",
    "com.apple.Passwords",
    "org.keepassxc.keepassxc",
];

/// The defaults in the first release, before `offered_exclusions` was recorded.
fn first_default_exclusions() -> Vec<String> {
    DEFAULT_EXCLUSIONS[..6].iter().map(|s| s.to_string()).collect()
}

const REVIEW_SHORTCUT: &str = "Command+E Command+E";
/// The first default, replaced because ⌃⌥E clashes with other apps.
const OLD_REVIEW_SHORTCUT: &str = "Control+Alt+E";

impl Default for Hotkeys {
    fn default() -> Self {
        // Hold ⌘ and press E twice. A single ⌘E still reaches the app, a moment later.
        Self { review: REVIEW_SHORTCUT.into(), quick_fix: "Control+Alt+Shift+E".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub providers: Vec<ProviderProfile>,
    pub active_provider: Option<String>,
    pub presets: Vec<Preset>,
    /// Built-in presets already added to `presets`, so a new built-in reaches existing settings
    /// once and one the user deleted stays deleted.
    #[serde(default)]
    pub offered_presets: Vec<String>,
    pub default_preset: String,
    /// Standing guidance applied to every request ("I write for engineers; no em dashes").
    pub style_notes: String,
    /// English spelling convention, e.g. "American English".
    pub dialect: String,
    pub hotkeys: Hotkeys,
    /// Show suggestions automatically when text is selected in an editable field.
    pub auto_popup: bool,
    /// Minimum selection length (characters) for automatic suggestions.
    pub auto_min_chars: usize,
    /// With nothing selected, the review shortcut and quick fix select the sentence before the
    /// caret and review that. Automatic suggestions never do.
    pub select_sentence: bool,
    /// Apply a proofread by changing only the words it fixes, where the app allows it, so the
    /// formatting around them is kept. Off, every fix replaces the whole selection.
    pub keep_formatting: bool,
    /// Names, places, and terms to keep exactly as written: a word ("Rhyan") or a phrase
    /// ("1234 Glenmoor Ct"). See `words.rs`.
    pub words: Vec<String>,
    /// Bundle identifiers where SuperEditr never reads text.
    pub excluded_apps: Vec<String>,
    /// Default exclusions already added to `excluded_apps`, so a new default reaches existing
    /// settings once and one the user removed stays removed. Settings from before this field
    /// existed had seen the first defaults.
    #[serde(default = "first_default_exclusions")]
    pub offered_exclusions: Vec<String>,
    pub show_timing: bool,
    /// Keep capture.log, a local record of each selection read and write (app IDs, timings and
    /// lengths, never text), for diagnosing a problem in a particular app. Off unless turned on.
    pub diagnostic_log: bool,
    pub onboarded: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            active_provider: None,
            presets: builtin_presets(),
            offered_presets: builtin_presets().into_iter().map(|p| p.id).collect(),
            default_preset: "proofread".into(),
            style_notes: String::new(),
            dialect: "American English".into(),
            hotkeys: Hotkeys::default(),
            auto_popup: true,
            auto_min_chars: 12,
            select_sentence: true,
            keep_formatting: true,
            words: Vec::new(),
            excluded_apps: DEFAULT_EXCLUSIONS.iter().map(|s| s.to_string()).collect(),
            offered_exclusions: DEFAULT_EXCLUSIONS.iter().map(|s| s.to_string()).collect(),
            show_timing: true,
            diagnostic_log: false,
            onboarded: false,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Self {
        let mut config: Config =
            std::fs::read_to_string(path).ok().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
        config.merge_new_builtins();
        config.merge_new_exclusions();
        config.drop_builtin_ai();
        if config.hotkeys.review == OLD_REVIEW_SHORTCUT {
            config.hotkeys.review = REVIEW_SHORTCUT.into();
        }
        config
    }

    /// Writes atomically so a crash mid-save never leaves a truncated settings file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(tmp, path)
    }

    /// Adds default exclusions introduced since these settings were saved.
    fn merge_new_exclusions(&mut self) {
        for app in DEFAULT_EXCLUSIONS {
            if !self.offered_exclusions.iter().any(|a| a == app) {
                self.offered_exclusions.push(app.to_string());
                if !self.is_excluded(app) {
                    self.excluded_apps.push(app.to_string());
                }
            }
        }
    }

    /// Apple Intelligence is built in rather than a provider the user adds; earlier versions listed
    /// it as one. Choosing it is now the same as choosing no other AI.
    fn drop_builtin_ai(&mut self) {
        let builtin: Vec<String> = self
            .providers
            .iter()
            .filter(|p| p.kind == crate::provider::ProviderKind::AppleIntelligence)
            .map(|p| p.id.clone())
            .collect();
        self.providers.retain(|p| !builtin.contains(&p.id));
        if self.active_provider.as_ref().is_some_and(|id| builtin.contains(id)) {
            self.active_provider = None;
        }
    }

    /// Adds built-in presets introduced after the user's settings were created, keeping edits and
    /// deletions. There is always at least one preset.
    fn merge_new_builtins(&mut self) {
        for builtin in builtin_presets() {
            if !self.offered_presets.contains(&builtin.id) {
                self.offered_presets.push(builtin.id.clone());
                if !self.presets.iter().any(|p| p.id == builtin.id) {
                    self.presets.push(builtin);
                }
            }
        }
        if self.presets.is_empty() {
            self.presets = builtin_presets();
        }
    }

    /// True when SuperEditr must never read text from this app.
    pub fn is_excluded(&self, bundle_id: &str) -> bool {
        !bundle_id.is_empty() && self.excluded_apps.iter().any(|b| b.eq_ignore_ascii_case(bundle_id))
    }

    pub fn active_profile(&self) -> Option<&ProviderProfile> {
        let id = self.active_provider.as_ref()?;
        self.providers.iter().find(|p| &p.id == id)
    }

    pub fn preset(&self, id: &str) -> Option<&Preset> {
        self.presets.iter().find(|p| p.id == id)
    }

    pub fn default_preset(&self) -> &Preset {
        self.preset(&self.default_preset).unwrap_or(&self.presets[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderKind;

    #[test]
    fn round_trips_and_keeps_user_edits() {
        let dir = std::env::temp_dir().join(format!("supereditr-test-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut config = Config::default();
        config.presets[0].instruction = "custom".into();
        // Settings from before built-ins were tracked, missing one added since.
        config.presets.retain(|p| p.id != "english");
        config.offered_presets.clear();
        let mut profile = ProviderProfile::new(ProviderKind::Anthropic);
        profile.api_key = Some("secret".into());
        config.providers.push(profile);
        config.save(&path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("secret"));
        let loaded = Config::load(&path);
        assert_eq!(loaded.presets[0].instruction, "custom");
        assert!(loaded.preset("english").is_some(), "new built-ins are merged back in");
        assert_eq!(loaded.providers[0].kind, ProviderKind::Anthropic);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn diagnostic_log_is_off_unless_turned_on() {
        assert!(!Config::default().diagnostic_log);
        let dir = std::env::temp_dir().join(format!("supereditr-log-{}", std::process::id()));
        let path = dir.join("config.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r#"{"onboarded": true}"#).unwrap();
        assert!(!Config::load(&path).diagnostic_log, "settings saved before the option existed keep no log");
        std::fs::write(&path, r#"{"diagnostic_log": true}"#).unwrap();
        assert!(Config::load(&path).diagnostic_log);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deleted_presets_stay_deleted_but_one_always_remains() {
        let dir = std::env::temp_dir().join(format!("supereditr-presets-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut config = Config::default();
        config.presets.retain(|p| p.id == "proofread");
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).presets.len(), 1, "a deleted built-in isn't added back");
        config.presets.clear();
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).presets, builtin_presets(), "never zero presets");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn apple_intelligence_profiles_become_the_builtin_default() {
        let dir = std::env::temp_dir().join(format!("supereditr-builtin-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut config = Config::default();
        config.hotkeys.review = OLD_REVIEW_SHORTCUT.into();
        let apple = ProviderProfile::new(ProviderKind::AppleIntelligence);
        config.active_provider = Some(apple.id.clone());
        config.providers = vec![ProviderProfile::new(ProviderKind::Anthropic), apple];
        config.save(&path).unwrap();
        let loaded = Config::load(&path);
        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.active_provider, None, "no other AI chosen, so Apple Intelligence reviews");
        assert_eq!(loaded.hotkeys.review, REVIEW_SHORTCUT, "the old default shortcut moves to the new one");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn new_default_exclusions_reach_existing_settings_once() {
        let dir = std::env::temp_dir().join(format!("supereditr-exclusions-{}", std::process::id()));
        let path = dir.join("settings.json");
        std::fs::create_dir_all(&dir).unwrap();
        // Saved by the first release, after the user removed iTerm from the list.
        let old = r#"{"excluded_apps": ["com.1password.1password", "com.nordsec.nordpass", "com.bitwarden.desktop",
            "com.apple.keychainaccess", "com.apple.Terminal"]}"#;
        std::fs::write(&path, old).unwrap();
        let mut loaded = Config::load(&path);
        assert!(loaded.is_excluded("com.cmuxterm.app"), "a new default is added");
        assert!(loaded.is_excluded("com.apple.Passwords"), "Apple's Passwords app is excluded");
        assert!(!loaded.is_excluded("com.googlecode.iterm2"), "a removed first-release default stays removed");
        // Removing a new default is remembered too.
        loaded.excluded_apps.retain(|a| a != "com.cmuxterm.app");
        loaded.save(&path).unwrap();
        assert!(!Config::load(&path).is_excluded("com.cmuxterm.app"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn excludes_password_managers_and_terminals_by_default() {
        let config = Config::default();
        assert!(config.is_excluded("com.1password.1password"));
        assert!(config.is_excluded("COM.APPLE.TERMINAL"));
        assert!(config.is_excluded("com.cmuxterm.app"));
        assert!(!config.is_excluded("com.apple.TextEdit"));
        assert!(!config.is_excluded(""));
    }

    #[test]
    fn missing_file_gives_defaults() {
        let config = Config::load(Path::new("/nonexistent/supereditr.json"));
        assert_eq!(config.default_preset().id, "proofread");
        assert!(config.auto_popup);
    }

    #[test]
    fn settings_saved_before_sentence_selection_turn_it_on() {
        let old: Config = serde_json::from_str(r#"{"auto_popup": false}"#).unwrap();
        assert!(old.select_sentence);
        let off: Config = serde_json::from_str(r#"{"select_sentence": false}"#).unwrap();
        assert!(!off.select_sentence);
    }

    #[test]
    fn settings_saved_before_keep_formatting_turn_it_on() {
        let old: Config = serde_json::from_str(r#"{"auto_popup": false}"#).unwrap();
        assert!(old.keep_formatting);
    }
}
