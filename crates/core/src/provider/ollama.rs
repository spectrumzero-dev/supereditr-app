use super::{ChatRequest, ProviderError, ProviderProfile, Providers, http_error, merge_extra, rejects_option};
use crate::sse::LineParser;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{Value, json};

const NAME: &str = "Ollama";

pub(super) async fn stream(
    providers: &Providers,
    profile: &ProviderProfile,
    request: &ChatRequest,
    on_delta: &mut (dyn FnMut(&str) + Send),
) -> Result<String, ProviderError> {
    let learned_key = format!("ollama:{}", profile.model);
    let mut variant = providers.learned_variant(&learned_key).min(1);
    loop {
        let mut body = json!({
            "model": profile.model,
            "messages": [
                {"role": "system", "content": request.system},
                {"role": "user", "content": request.user},
            ],
            "stream": true,
            // Keep the model resident so the next check doesn't pay the load time.
            "keep_alive": "30m",
            "options": {"temperature": 0.2, "num_predict": request.max_tokens},
        });
        if variant == 0 {
            body["think"] = json!(false);
        }
        merge_extra(&mut body, &profile.extra_body);
        let response = providers.http().post(profile.url("/api/chat")).json(&body).send().await?;
        if !response.status().is_success() {
            let error = http_error(NAME, &profile.model, response).await;
            if variant == 0 && rejects_option(&error, &["think"]) {
                variant = 1;
                providers.learn_variant(&learned_key, 1);
                continue;
            }
            return Err(error);
        }
        let mut parser = LineParser::default();
        let mut output = String::new();
        let mut bytes = response.bytes_stream();
        while let Some(chunk) = bytes.next().await {
            for line in parser.push(&chunk?) {
                let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
                if let Some(error) = value["error"].as_str() {
                    return Err(ProviderError::Protocol(error.to_string()));
                }
                if let Some(text) = value.pointer("/message/content").and_then(Value::as_str) {
                    output.push_str(text);
                    on_delta(text);
                }
                if value["done"].as_bool() == Some(true) {
                    return match value["done_reason"].as_str() {
                        Some("length") => Err(ProviderError::Truncated),
                        _ => Ok(output),
                    };
                }
            }
        }
        return Err(ProviderError::Incomplete);
    }
}

pub(super) async fn list_models(
    providers: &Providers,
    profile: &ProviderProfile,
) -> Result<Vec<String>, ProviderError> {
    let response = providers.http().get(profile.url("/api/tags")).send().await?;
    if !response.status().is_success() {
        return Err(http_error(NAME, &profile.model, response).await);
    }
    let value: Value = response.json().await?;
    Ok(value["models"]
        .as_array()
        .map(|items| items.iter().filter_map(|m| m["name"].as_str().map(str::to_string)).collect())
        .unwrap_or_default())
}

#[derive(Debug, Clone, Serialize)]
pub struct PullProgress {
    pub status: String,
    pub completed: u64,
    pub total: u64,
}

impl Providers {
    /// Downloads a model into a running Ollama server, reporting progress as it goes.
    pub async fn ollama_pull(
        &self,
        base_url: &str,
        model: &str,
        on_progress: &mut (dyn FnMut(PullProgress) + Send),
    ) -> Result<(), ProviderError> {
        let url = format!("{}/api/pull", base_url.trim_end_matches('/'));
        let response = self
            .http()
            .post(url)
            .timeout(std::time::Duration::from_secs(60 * 60))
            .json(&json!({"model": model, "stream": true}))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(http_error(NAME, model, response).await);
        }
        let mut parser = LineParser::default();
        let mut bytes = response.bytes_stream();
        while let Some(chunk) = bytes.next().await {
            for line in parser.push(&chunk?) {
                let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
                if let Some(error) = value["error"].as_str() {
                    return Err(ProviderError::Protocol(error.to_string()));
                }
                on_progress(PullProgress {
                    status: value["status"].as_str().unwrap_or_default().to_string(),
                    completed: value["completed"].as_u64().unwrap_or(0),
                    total: value["total"].as_u64().unwrap_or(0),
                });
            }
        }
        Ok(())
    }
}
