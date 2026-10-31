//! Model providers. Each one streams plain text deltas for a single system + user prompt.
//!
//! Providers are called over raw HTTP with a shared, connection-pooled client so a warmed
//! TLS/HTTP2 connection can be reused for the next request. Latency-sensitive defaults (thinking
//! off, low reasoning effort) are applied per provider and relaxed automatically when a model
//! rejects them.

mod anthropic;
mod apple;
mod cli;
mod gemini;
mod ollama;
mod openai;

pub use apple::{AppleStatus, helper_path as apple_helper_path};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    Gemini,
    Ollama,
    /// Any server that speaks the OpenAI Chat Completions API (LM Studio, OpenRouter, Groq…).
    OpenAiCompatible,
    /// Uses an installed Claude Code CLI and the subscription it is signed in with.
    ClaudeCli,
    /// Apple's on-device model (macOS 26+ with Apple Intelligence), through a bundled helper.
    AppleIntelligence,
}

impl ProviderKind {
    pub fn needs_key(self) -> bool {
        matches!(self, Self::Anthropic | Self::OpenAi | Self::Gemini)
    }

    /// Whether the offline checker goes over this AI's revision as a final pass. Apple's on-device
    /// model and local models can miss mistakes its rules catch ("should of"), and an
    /// OpenAI-compatible server may be running one. The cloud AIs catch those themselves, and read
    /// context the rules can't: a name the dictionary also lists as an acronym ("Sam", "SAM") stays
    /// a name.
    pub fn needs_final_pass(self) -> bool {
        matches!(self, Self::AppleIntelligence | Self::Ollama | Self::OpenAiCompatible)
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com",
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Gemini => "https://generativelanguage.googleapis.com",
            Self::Ollama => "http://127.0.0.1:11434",
            Self::OpenAiCompatible => "http://127.0.0.1:1234/v1",
            Self::ClaudeCli => "claude",
            // Empty means "the helper bundled next to SuperEditr".
            Self::AppleIntelligence => "",
        }
    }

    /// Fast, inexpensive defaults; the UI offers every model the account can list.
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Anthropic => "claude-haiku-5-5",
            Self::OpenAi => "gpt-6-luna",
            Self::Gemini => "gemini-3.5-flash-lite",
            Self::Ollama => "gemma3:4b",
            Self::OpenAiCompatible => "",
            Self::ClaudeCli => "haiku",
            Self::AppleIntelligence => "system",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Anthropic => "Claude (Anthropic API)",
            Self::OpenAi => "OpenAI",
            Self::Gemini => "Google Gemini",
            Self::Ollama => "Ollama (local)",
            Self::OpenAiCompatible => "OpenAI-compatible server",
            Self::ClaudeCli => "Claude Code subscription",
            Self::AppleIntelligence => "Apple Intelligence (on this Mac)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProfile {
    pub id: String,
    pub kind: ProviderKind,
    pub label: String,
    pub base_url: String,
    pub model: String,
    /// Extra JSON merged into each request body, for provider options SuperEditr does not model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_body: Option<serde_json::Value>,
    /// Loaded from the OS keychain at runtime; never written to the settings file.
    #[serde(skip)]
    pub api_key: Option<String>,
}

impl ProviderProfile {
    pub fn new(kind: ProviderKind) -> Self {
        Self {
            id: format!("{kind:?}").to_lowercase(),
            kind,
            label: kind.display_name().to_string(),
            base_url: kind.default_base_url().to_string(),
            model: kind.default_model().to_string(),
            extra_body: None,
            api_key: None,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub system: String,
    pub user: String,
    pub max_tokens: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("Add an API key for {0} in Settings.")]
    MissingKey(&'static str),
    #[error("The provider rejected the API key. Check it in Settings.")]
    Unauthorized,
    #[error("Rate limited by the provider. Try again in a moment.")]
    RateLimited,
    #[error("Model “{0}” was not found. Pick another model in Settings.")]
    ModelNotFound(String),
    #[error("{provider} returned HTTP {status}: {message}")]
    Http { provider: &'static str, status: u16, message: String },
    #[error("The model stopped at its length limit before finishing. Select less text and try again.")]
    Truncated,
    #[error("The response ended before the model finished. Try again.")]
    Incomplete,
    #[error("The model declined to edit this text. Try another model.")]
    Declined,
    #[error("Couldn’t reach the provider: {0}")]
    Network(String),
    #[error("Unexpected response from the provider: {0}")]
    Protocol(String),
    #[error("{0}")]
    Process(String),
}

impl From<reqwest::Error> for ProviderError {
    fn from(error: reqwest::Error) -> Self {
        let message = if error.is_timeout() {
            "the request timed out".to_string()
        } else if error.is_connect() {
            "connection failed".to_string()
        } else {
            error.without_url().to_string()
        };
        Self::Network(message)
    }
}

/// Shared HTTP client plus per-model knowledge learned at runtime (for example, that a model
/// rejects `reasoning_effort: "none"`).
#[derive(Clone)]
pub struct Providers {
    client: reqwest::Client,
    learned: Arc<Mutex<HashMap<String, usize>>>,
    /// The resident Apple Intelligence helper, started on first use.
    apple: Arc<tokio::sync::Mutex<Option<Arc<apple::Helper>>>>,
}

impl Default for Providers {
    fn default() -> Self {
        Self::new()
    }
}

impl Providers {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(30))
            .tcp_nodelay(true)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("SuperEditr/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("HTTP client configuration is valid");
        Self { client, learned: Arc::default(), apple: Arc::default() }
    }

    /// Streams a completion, calling `on_delta` with each text fragment, and returns the full text.
    pub async fn stream(
        &self,
        profile: &ProviderProfile,
        request: &ChatRequest,
        on_delta: &mut (dyn FnMut(&str) + Send),
    ) -> Result<String, ProviderError> {
        match profile.kind {
            ProviderKind::Anthropic => anthropic::stream(self, profile, request, on_delta).await,
            ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => {
                openai::stream(self, profile, request, on_delta).await
            }
            ProviderKind::Gemini => gemini::stream(self, profile, request, on_delta).await,
            ProviderKind::Ollama => ollama::stream(self, profile, request, on_delta).await,
            ProviderKind::ClaudeCli => cli::stream_claude(profile, request, on_delta).await,
            ProviderKind::AppleIntelligence => apple::stream(self, profile, request, on_delta).await,
        }
    }

    pub async fn list_models(&self, profile: &ProviderProfile) -> Result<Vec<String>, ProviderError> {
        let mut models = match profile.kind {
            ProviderKind::Anthropic => anthropic::list_models(self, profile).await?,
            ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => openai::list_models(self, profile).await?,
            ProviderKind::Gemini => gemini::list_models(self, profile).await?,
            ProviderKind::Ollama => ollama::list_models(self, profile).await?,
            ProviderKind::ClaudeCli => vec!["haiku".into(), "sonnet".into(), "opus".into()],
            ProviderKind::AppleIntelligence => apple::list_models(self, profile).await?,
        };
        models.sort();
        models.dedup();
        Ok(models)
    }

    /// Whether an AI can answer reviews: its key is present and its server lists models within a
    /// few seconds, or for the Claude CLI, the program runs. `Err` says what to fix.
    pub async fn check(&self, profile: &ProviderProfile) -> Result<Vec<String>, String> {
        if profile.kind.needs_key() && profile.api_key.as_deref().is_none_or(|k| k.trim().is_empty()) {
            return Err("Add an API key".into());
        }
        if profile.kind == ProviderKind::ClaudeCli {
            // `--version` works signed out too; `auth status` reports whether reviews can run.
            let program = if profile.base_url.is_empty() { "claude" } else { profile.base_url.as_str() };
            let ran = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                tokio::process::Command::new(program).args(["auth", "status"]).kill_on_drop(true).output(),
            )
            .await;
            let Ok(Ok(out)) = ran else { return Err(format!("Claude Code didn’t run at {program}")) };
            let status: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
            return match status["loggedIn"].as_bool() {
                Some(true) => Ok(vec!["haiku".into(), "sonnet".into(), "opus".into()]),
                Some(false) => Err("Sign in to Claude Code: run `claude` in Terminal and use /login".into()),
                None => Err(format!("Claude Code at {program} didn’t report whether it’s signed in")),
            };
        }
        match tokio::time::timeout(std::time::Duration::from_secs(8), self.list_models(profile)).await {
            Ok(Ok(models)) if models.is_empty() => Err(if profile.kind == ProviderKind::Ollama {
                "No models downloaded yet: download one below".into()
            } else {
                "This server lists no models".into()
            }),
            Ok(Ok(models)) => Ok(models),
            Ok(Err(e)) => Err(format!("Can’t connect: {}", e.to_string().trim_end_matches('.'))),
            Err(_) => Err("Can’t connect: no answer in 8 seconds".into()),
        }
    }

    /// Opens (or refreshes) a pooled connection so the next real request skips DNS, TCP, and
    /// TLS setup. Errors are irrelevant here; any response means the connection is warm.
    pub async fn warm(&self, profile: &ProviderProfile) {
        match profile.kind {
            ProviderKind::ClaudeCli => return,
            ProviderKind::AppleIntelligence => return apple::warm(self, profile).await,
            _ => {}
        }
        let _ = self.client.head(profile.url("/")).timeout(Duration::from_secs(3)).send().await;
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.client
    }

    fn learned_variant(&self, key: &str) -> usize {
        self.learned.lock().map(|m| m.get(key).copied().unwrap_or(0)).unwrap_or(0)
    }

    fn learn_variant(&self, key: &str, variant: usize) {
        if let Ok(mut m) = self.learned.lock() {
            m.insert(key.to_string(), variant);
        }
    }
}

/// Converts a non-success HTTP response into a typed error.
async fn http_error(provider: &'static str, model: &str, response: reqwest::Response) -> ProviderError {
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    let message = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.pointer("/error"))
                .or_else(|| v.pointer("/message"))
                .and_then(|m| m.as_str().map(str::to_string))
        })
        .unwrap_or_else(|| body.chars().take(300).collect());
    match status {
        401 | 403 => ProviderError::Unauthorized,
        429 => ProviderError::RateLimited,
        404 => ProviderError::ModelNotFound(model.to_string()),
        _ => ProviderError::Http { provider, status, message },
    }
}

/// True when a 400 response complains about one of the optional latency parameters, which
/// means the request should be retried with the next, more conservative variant.
fn rejects_option(error: &ProviderError, needles: &[&str]) -> bool {
    match error {
        ProviderError::Http { status: 400 | 422, message, .. } => {
            let lower = message.to_lowercase();
            needles.iter().any(|n| lower.contains(n))
        }
        _ => false,
    }
}

fn merge_extra(body: &mut serde_json::Value, extra: &Option<serde_json::Value>) {
    if let (Some(serde_json::Value::Object(extra)), serde_json::Value::Object(target)) = (extra, body) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
}

/// Picks the preferred model from an account's model list, falling back to the default.
pub fn preferred_model(kind: ProviderKind, available: &[String]) -> String {
    let preferences: &[&str] = match kind {
        ProviderKind::Anthropic => &["claude-haiku-5-5", "claude-haiku-4-5", "claude-sonnet-5-5"],
        ProviderKind::OpenAi => &["gpt-6-luna", "gpt-5-mini", "gpt-4.1-mini", "gpt-4o-mini"],
        ProviderKind::Gemini => return newest_gemini(available).unwrap_or_else(|| kind.default_model().to_string()),
        ProviderKind::Ollama => &["gemma3:4b", "qwen3:4b", "llama3.2:3b", "gemma3", "qwen3", "llama3.2", "mistral"],
        _ => &[],
    };
    for preference in preferences {
        if let Some(found) =
            available.iter().find(|m| m.as_str() == *preference || m.starts_with(&format!("{preference}:")))
        {
            return found.clone();
        }
    }
    available.first().cloned().unwrap_or_else(|| kind.default_model().to_string())
}

/// Newest stable "flash-lite" model, else newest "flash": the fastest Gemini tiers.
fn newest_gemini(available: &[String]) -> Option<String> {
    let version = |m: &str| -> Vec<u32> {
        m.trim_start_matches("gemini-")
            .split('-')
            .next()
            .unwrap_or("")
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect()
    };
    let usable = |m: &&String| !["preview", "exp", "image", "tts", "live", "audio"].iter().any(|x| m.contains(x));
    for tier in ["-flash-lite", "-flash"] {
        if let Some(best) =
            available.iter().filter(usable).filter(|m| m.ends_with(tier)).max_by(|a, b| version(a).cmp(&version(b)))
        {
            return Some(best.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_newest_fast_gemini() {
        let models: Vec<String> = [
            "gemini-2.5-flash-lite",
            "gemini-3.5-flash-lite",
            "gemini-3.1-flash-lite-image",
            "gemini-3.8-flash",
            "gemini-4-argon",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(preferred_model(ProviderKind::Gemini, &models), "gemini-3.5-flash-lite");
        let flash_only: Vec<String> =
            ["gemini-3.7-flash", "gemini-3.8-flash", "gemini-3-flash-preview"].iter().map(|s| s.to_string()).collect();
        assert_eq!(preferred_model(ProviderKind::Gemini, &flash_only), "gemini-3.8-flash");
    }

    #[test]
    fn preferred_model_falls_back_in_order() {
        let models = vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()];
        assert_eq!(preferred_model(ProviderKind::OpenAi, &models), "gpt-4o-mini");
        assert_eq!(preferred_model(ProviderKind::Ollama, &["llama3.2:3b".into()]), "llama3.2:3b");
        assert_eq!(preferred_model(ProviderKind::Gemini, &[]), "gemini-3.5-flash-lite");
    }

    #[test]
    fn profile_never_serializes_key() {
        let mut profile = ProviderProfile::new(ProviderKind::Anthropic);
        profile.api_key = Some("secret".into());
        let json = serde_json::to_string(&profile).unwrap();
        assert!(!json.contains("secret"));
    }
}
