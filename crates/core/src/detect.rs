//! Finds AI accounts and local models the user already has, so setup is one click.
//!
//! Detection only runs when the user asks for it (onboarding or Settings). Keys found here stay
//! in memory until the user chooses one; the app then stores it in the OS keychain.

use crate::provider::{ProviderKind, Providers};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub kind: ProviderKind,
    /// Where it was found, e.g. "ANTHROPIC_API_KEY in your shell profile".
    pub source: String,
    #[serde(skip)]
    pub api_key: Option<String>,
    /// Masked key for display, e.g. "sk-ant-…9f2c".
    pub key_hint: Option<String>,
    pub base_url: String,
    pub models: Vec<String>,
    pub note: Option<String>,
    /// False when it was found but can't be used yet (for example, a server that isn't running).
    pub ready: bool,
}

impl Candidate {
    fn new(kind: ProviderKind, source: impl Into<String>) -> Self {
        Self {
            kind,
            source: source.into(),
            api_key: None,
            key_hint: None,
            base_url: kind.default_base_url().to_string(),
            models: Vec::new(),
            note: None,
            ready: true,
        }
    }

    fn with_key(mut self, key: &str) -> Self {
        self.key_hint = Some(mask(key));
        self.api_key = Some(key.to_string());
        self
    }
}

pub fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 10 {
        return "••••".into();
    }
    let prefix: String = chars.iter().take_while(|c| **c != '-').take(6).collect();
    let prefix = if chars.get(prefix.len()) == Some(&'-') { format!("{prefix}-") } else { String::new() };
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    format!("{prefix}…{suffix}")
}

const ENV_KEYS: &[(&str, ProviderKind)] = &[
    ("ANTHROPIC_API_KEY", ProviderKind::Anthropic),
    ("OPENAI_API_KEY", ProviderKind::OpenAi),
    ("GEMINI_API_KEY", ProviderKind::Gemini),
    ("GOOGLE_API_KEY", ProviderKind::Gemini),
];

/// API keys exported as environment variables.
pub fn env_candidates(env: &HashMap<String, String>, origin: &str) -> Vec<Candidate> {
    ENV_KEYS
        .iter()
        .filter_map(|(name, kind)| {
            let value = env.get(*name)?.trim();
            (value.len() > 8).then(|| Candidate::new(*kind, format!("{name} in {origin}")).with_key(value))
        })
        .collect()
}

/// An OpenAI API key saved by the Codex CLI when it was set up with a key rather than a
/// ChatGPT sign-in. ChatGPT sign-in tokens are not usable by other apps and are ignored.
pub fn codex_candidate(home: &Path) -> Option<Candidate> {
    let raw = std::fs::read_to_string(home.join(".codex/auth.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let key = value["OPENAI_API_KEY"].as_str()?.trim();
    (key.len() > 8)
        .then(|| Candidate::new(ProviderKind::OpenAi, "Codex CLI settings (~/.codex/auth.json)").with_key(key))
}

pub fn find_executable(name: &str, path_var: &str, home: &Path) -> Option<PathBuf> {
    let extra = [
        home.join(".local/bin"),
        home.join(".claude/local"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    std::env::split_paths(path_var).chain(extra).map(|dir| dir.join(name)).find(|candidate| candidate.is_file())
}

/// Environment of the user's login shell. Apps launched from Finder don't inherit exports from
/// `~/.zshrc`, which is where most people keep API keys.
pub async fn login_shell_env() -> HashMap<String, String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let output = tokio::time::timeout(
        Duration::from_secs(4),
        tokio::process::Command::new(shell).args(["-ilc", "env -0"]).kill_on_drop(true).output(),
    )
    .await;
    let Ok(Ok(output)) = output else { return HashMap::new() };
    output
        .stdout
        .split(|b| *b == 0)
        .filter_map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            let (key, value) = entry.split_once('=')?;
            Some((key.trim().to_string(), value.to_string()))
        })
        .collect()
}

async fn probe_local(providers: &Providers, kind: ProviderKind, base_url: &str) -> Option<Candidate> {
    let mut profile = crate::provider::ProviderProfile::new(kind);
    profile.base_url = base_url.to_string();
    let models = tokio::time::timeout(Duration::from_millis(800), providers.list_models(&profile)).await.ok()?.ok()?;
    let mut candidate = Candidate::new(kind, format!("running at {base_url}"));
    candidate.base_url = base_url.to_string();
    if models.is_empty() {
        candidate.note = Some("Running, but no models are downloaded yet.".into());
    }
    candidate.models = models;
    Some(candidate)
}

/// Apple's on-device model, when the bundled helper is present. Offered even when it can't be used
/// yet, with the reason, because turning on Apple Intelligence is a one-click fix.
async fn apple_candidate(providers: &Providers) -> Option<Candidate> {
    let profile = crate::provider::ProviderProfile::new(ProviderKind::AppleIntelligence);
    crate::provider::apple_helper_path(&profile.base_url)?;
    let status = providers.apple_status(&profile).await;
    if status.reason.as_deref().is_some_and(|r| r == "osTooOld" || r == "deviceNotEligible" || r == "missing") {
        return None;
    }
    let mut candidate = Candidate::new(ProviderKind::AppleIntelligence, "built into macOS");
    candidate.ready = status.available;
    candidate.note = Some(if status.available {
        "Private and free: runs on this Mac with no download or key.".into()
    } else {
        status.message().unwrap_or_default()
    });
    Some(candidate)
}

/// Runs every detector concurrently. Typically finishes in well under a second, bounded by the
/// login-shell read.
pub async fn scan(providers: &Providers, home: &Path) -> Vec<Candidate> {
    let process_env: HashMap<String, String> = std::env::vars().collect();
    let (shell_env, apple, ollama, lmstudio) = tokio::join!(
        login_shell_env(),
        apple_candidate(providers),
        probe_local(providers, ProviderKind::Ollama, ProviderKind::Ollama.default_base_url()),
        probe_local(providers, ProviderKind::OpenAiCompatible, "http://127.0.0.1:1234/v1"),
    );

    let mut found = env_candidates(&process_env, "the app environment");
    found.extend(env_candidates(&shell_env, "your shell profile"));
    found.extend(codex_candidate(home));

    found.extend(apple);

    let path_var = shell_env.get("PATH").or(process_env.get("PATH")).cloned().unwrap_or_default();
    if let Some(claude) = find_executable("claude", &path_var, home) {
        let mut candidate = Candidate::new(ProviderKind::ClaudeCli, format!("Claude Code at {}", claude.display()));
        candidate.base_url = claude.display().to_string();
        candidate.note = Some("Uses your Claude subscription. Slower to start (about 2–6 s) than an API key.".into());
        found.push(candidate);
    }
    if let Some(mut lm) = lmstudio {
        lm.note.get_or_insert_with(|| "LM Studio or another local OpenAI-compatible server.".into());
        found.push(lm);
    }
    match ollama {
        Some(candidate) => found.push(candidate),
        None => {
            if let Some(path) = find_executable("ollama", &path_var, home) {
                let mut candidate = Candidate::new(ProviderKind::Ollama, format!("installed at {}", path.display()));
                candidate.note = Some("Installed but not running. Start Ollama, then scan again.".into());
                candidate.ready = false;
                found.push(candidate);
            }
        }
    }

    // The same key is often both exported and saved by a CLI; show it once.
    let mut seen = std::collections::HashSet::new();
    found.retain(|c| match &c.api_key {
        Some(key) => seen.insert((c.kind, key.clone())),
        None => true,
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_keys() {
        assert_eq!(mask("sk-ant-api03-abcdefghijklmnop1234"), "sk-…1234");
        assert_eq!(mask("AIzaSyAbcdefghijklmnop9876"), "…9876");
        assert_eq!(mask("short"), "••••");
    }

    #[test]
    fn finds_env_keys() {
        let env = HashMap::from([
            ("ANTHROPIC_API_KEY".to_string(), "sk-ant-0123456789abcdef".to_string()),
            ("OPENAI_API_KEY".to_string(), "".to_string()),
        ]);
        let found = env_candidates(&env, "test");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, ProviderKind::Anthropic);
        assert!(found[0].api_key.is_some());
        let json = serde_json::to_string(&found[0]).unwrap();
        assert!(!json.contains("0123456789abcdef"), "keys never reach the UI");
    }

    #[test]
    fn reads_codex_api_key_only() {
        let dir = std::env::temp_dir().join(format!("supereditr-codex-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".codex")).unwrap();
        std::fs::write(dir.join(".codex/auth.json"), r#"{"OPENAI_API_KEY": null, "tokens": {"access_token": "x"}}"#)
            .unwrap();
        assert!(codex_candidate(&dir).is_none());
        std::fs::write(dir.join(".codex/auth.json"), r#"{"OPENAI_API_KEY": "sk-proj-0123456789"}"#).unwrap();
        assert_eq!(codex_candidate(&dir).unwrap().kind, ProviderKind::OpenAi);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
