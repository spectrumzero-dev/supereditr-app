//! Apple's on-device model (FoundationModels) through the `supereditr-fm` helper.
//!
//! FoundationModels is Swift-only, so SuperEditr runs a small Swift helper once and keeps it alive,
//! exchanging JSON lines (see helpers/foundation-models/main.swift). Keeping it resident avoids
//! process start-up per review, and `warm` asks it to load the model ahead of the first request.

use super::{ChatRequest, ProviderError, ProviderProfile, Providers};
use crate::sse::LineParser;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

const HELPER_NAME: &str = "supereditr-fm";
/// The on-device model has a small context window; keep answers well inside it.
const MAX_RESPONSE_TOKENS: u32 = 2_000;

/// Whether the on-device model can be used, as reported by the helper.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AppleStatus {
    pub available: bool,
    pub reason: Option<String>,
}

impl AppleStatus {
    pub fn message(&self) -> Option<String> {
        self.reason.as_deref().map(reason_message)
    }
}

pub fn reason_message(code: &str) -> String {
    match code {
        "deviceNotEligible" => "This Mac doesn’t support Apple Intelligence.".into(),
        "appleIntelligenceNotEnabled" => {
            "Turn on Apple Intelligence in System Settings → Apple Intelligence & Siri.".into()
        }
        "modelNotReady" => "Apple Intelligence is still downloading its model. Try again later.".into(),
        "osTooOld" => "Apple’s on-device model needs macOS 26 or later.".into(),
        "contextWindow" => {
            "That selection is too long for Apple’s on-device model. Select less text or use another model.".into()
        }
        "declined" => "Apple’s on-device model declined this text. Try another model.".into(),
        "language" => "Apple’s on-device model doesn’t support this language yet.".into(),
        "missing" => "The Apple Intelligence helper isn’t installed with this copy of SuperEditr.".into(),
        _ => "Apple’s on-device model isn’t available right now.".into(),
    }
}

/// Whether a profile path or `SUPEREDITR_FM_HELPER` may replace the bundled helper. Official
/// release builds (`SUPEREDITR_RELEASE_BUILD` at compile time) refuse: the helper runs with
/// SuperEditr's Accessibility permission, so a path another program can set must not choose it.
const HELPER_OVERRIDES: bool = option_env!("SUPEREDITR_RELEASE_BUILD").is_none();

/// Finds the helper: an explicit path from the profile, `SUPEREDITR_FM_HELPER`, or the file
/// next to the running executable (where the app bundle and `cargo build` put it). Official
/// release builds use only the file next to the executable.
pub fn helper_path(configured: &str) -> Option<PathBuf> {
    if HELPER_OVERRIDES && !configured.trim().is_empty() {
        return Some(PathBuf::from(configured.trim()));
    }
    if let Some(path) = std::env::var("SUPEREDITR_FM_HELPER").ok().filter(|_| HELPER_OVERRIDES) {
        return Some(PathBuf::from(path));
    }
    let beside = std::env::current_exe().ok()?.parent()?.join(HELPER_NAME);
    beside.is_file().then_some(beside)
}

type Routes = Arc<Mutex<HashMap<u64, mpsc::UnboundedSender<Value>>>>;

/// A running helper process.
pub(super) struct Helper {
    lines: mpsc::UnboundedSender<String>,
    routes: Routes,
    status: watch::Receiver<Option<AppleStatus>>,
    alive: Arc<AtomicBool>,
    next_id: AtomicU64,
    path: PathBuf,
    _child: tokio::process::Child,
}

impl Helper {
    fn spawn(path: PathBuf) -> Result<Arc<Self>, ProviderError> {
        let mut child = Command::new(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| ProviderError::Process(format!("Couldn’t start the Apple Intelligence helper: {e}")))?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let (lines, mut outgoing) = mpsc::unbounded_channel::<String>();
        let routes: Routes = Arc::default();
        let (status_tx, status) = watch::channel(None);
        let alive = Arc::new(AtomicBool::new(true));

        tokio::spawn(async move {
            while let Some(line) = outgoing.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() || stdin.write_all(b"\n").await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });

        let reader_routes = routes.clone();
        let reader_alive = alive.clone();
        tokio::spawn(async move {
            let mut parser = LineParser::default();
            let mut buffer = [0u8; 8192];
            loop {
                let read = match stdout.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                for line in parser.push(&buffer[..read]) {
                    let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
                    if message["type"] == "hello" {
                        let _ = status_tx.send(Some(parse_status(&message)));
                        continue;
                    }
                    let Some(id) = message["id"].as_u64() else { continue };
                    if let Some(route) = reader_routes.lock().unwrap().get(&id) {
                        let _ = route.send(message);
                    }
                }
            }
            // Dropping every route ends any request still waiting.
            reader_alive.store(false, Ordering::Relaxed);
            reader_routes.lock().unwrap().clear();
            let _ = status_tx.send(Some(AppleStatus { available: false, reason: Some("stopped".into()) }));
        });

        Ok(Arc::new(Self { lines, routes, status, alive, next_id: AtomicU64::new(1), path, _child: child }))
    }

    /// Sends a message that expects replies, returning a guard that receives them and cancels the
    /// request if it is dropped early (for example, when the user moves on mid-review).
    fn call(self: &Arc<Self>, mut message: Value) -> Pending {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        message["id"] = json!(id);
        let (tx, rx) = mpsc::unbounded_channel();
        self.routes.lock().unwrap().insert(id, tx);
        let _ = self.lines.send(message.to_string());
        Pending { id, rx, helper: self.clone(), finished: false }
    }

    fn send(&self, message: Value) {
        let _ = self.lines.send(message.to_string());
    }

    /// Asks the helper now, because availability changes while SuperEditr runs (Apple Intelligence
    /// turned on, its model finishing a download). Falls back to the startup report.
    async fn status(self: &Arc<Self>) -> AppleStatus {
        if self.alive.load(Ordering::Relaxed) {
            let mut pending = self.call(json!({"type": "status"}));
            if let Ok(Some(reply)) = tokio::time::timeout(Duration::from_secs(1), pending.rx.recv()).await {
                pending.finished = true;
                return parse_status(&reply);
            }
            pending.finished = true;
        }
        let mut status = self.status.clone();
        let waited = tokio::time::timeout(Duration::from_secs(3), status.wait_for(Option::is_some)).await;
        match waited {
            Ok(Ok(value)) => value.clone().unwrap_or_else(|| missing("unavailable")),
            _ => missing("unavailable"),
        }
    }
}

struct Pending {
    id: u64,
    rx: mpsc::UnboundedReceiver<Value>,
    helper: Arc<Helper>,
    finished: bool,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.helper.routes.lock().unwrap().remove(&self.id);
        if !self.finished {
            self.helper.send(json!({"type": "cancel", "id": self.id}));
        }
    }
}

fn parse_status(message: &Value) -> AppleStatus {
    AppleStatus {
        available: message["available"].as_bool().unwrap_or(false),
        reason: message["reason"].as_str().map(str::to_string),
    }
}

fn missing(reason: &str) -> AppleStatus {
    AppleStatus { available: false, reason: Some(reason.into()) }
}

fn error_for(code: &str) -> ProviderError {
    match code {
        "rateLimited" => ProviderError::RateLimited,
        other => ProviderError::Process(reason_message(other)),
    }
}

impl Providers {
    /// The running helper, started on first use and restarted if it has exited.
    pub(super) async fn apple_helper(&self, profile: &ProviderProfile) -> Result<Arc<Helper>, ProviderError> {
        let path = helper_path(&profile.base_url).ok_or_else(|| ProviderError::Process(reason_message("missing")))?;
        let mut slot = self.apple.lock().await;
        if let Some(helper) = slot.as_ref().filter(|h| h.alive.load(Ordering::Relaxed) && h.path == path) {
            return Ok(helper.clone());
        }
        let helper = Helper::spawn(path)?;
        *slot = Some(helper.clone());
        Ok(helper)
    }

    pub async fn apple_status(&self, profile: &ProviderProfile) -> AppleStatus {
        match self.apple_helper(profile).await {
            Ok(helper) => helper.status().await,
            Err(_) => missing("missing"),
        }
    }
}

pub(super) async fn stream(
    providers: &Providers,
    profile: &ProviderProfile,
    request: &ChatRequest,
    on_delta: &mut (dyn FnMut(&str) + Send),
) -> Result<String, ProviderError> {
    let helper = providers.apple_helper(profile).await?;
    let mut pending = helper.call(json!({
        "type": "request",
        "system": request.system,
        "user": request.user,
        "maxTokens": request.max_tokens.min(MAX_RESPONSE_TOKENS),
    }));
    let mut output = String::new();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(60), pending.rx.recv())
            .await
            .map_err(|_| ProviderError::Network("Apple’s on-device model timed out".into()))?
            .ok_or_else(|| ProviderError::Process("The Apple Intelligence helper stopped unexpectedly.".into()))?;
        match message["type"].as_str() {
            Some("delta") => {
                if let Some(text) = message["text"].as_str() {
                    output.push_str(text);
                    on_delta(text);
                }
            }
            Some("done") => {
                pending.finished = true;
                // The final text is authoritative: the model can revise earlier output mid-stream.
                return Ok(message["text"].as_str().map(str::to_string).unwrap_or(output));
            }
            Some("error") => {
                pending.finished = true;
                return Err(error_for(message["code"].as_str().unwrap_or("")));
            }
            _ => {}
        }
    }
}

pub(super) async fn list_models(
    providers: &Providers,
    profile: &ProviderProfile,
) -> Result<Vec<String>, ProviderError> {
    let status = providers.apple_status(profile).await;
    if status.available {
        Ok(vec!["system".into()])
    } else {
        Err(ProviderError::Process(status.message().unwrap_or_else(|| reason_message(""))))
    }
}

/// Starts the helper and asks it to load the model, so the first review doesn't pay for it.
pub(super) async fn warm(providers: &Providers, profile: &ProviderProfile) {
    if let Ok(helper) = providers.apple_helper(profile).await {
        helper.send(json!({"type": "prewarm"}));
    }
}
