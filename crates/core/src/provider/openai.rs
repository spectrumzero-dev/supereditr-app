use super::{
    ChatRequest, ProviderError, ProviderKind, ProviderProfile, Providers, http_error, merge_extra, rejects_option,
};
use crate::sse::SseParser;
use futures_util::StreamExt;
use serde_json::{Value, json};

fn is_reasoning_model(model: &str) -> bool {
    let m = model.rsplit('/').next().unwrap_or(model);
    m.starts_with("gpt-5")
        || m.starts_with("gpt-6")
        || m.starts_with("o1")
        || m.starts_with("o3")
        || m.starts_with("o4")
}

/// Reasoning-effort settings to try, fastest first. Model generations accept different
/// minimums, so the first one a model rejects is skipped from then on.
fn effort_variants(profile: &ProviderProfile) -> Vec<Option<&'static str>> {
    if profile.kind == ProviderKind::OpenAi && is_reasoning_model(&profile.model) {
        vec![Some("none"), Some("minimal"), Some("low"), None]
    } else {
        vec![None]
    }
}

pub(super) async fn stream(
    providers: &Providers,
    profile: &ProviderProfile,
    request: &ChatRequest,
    on_delta: &mut (dyn FnMut(&str) + Send),
) -> Result<String, ProviderError> {
    let name = if profile.kind == ProviderKind::OpenAi { "OpenAI" } else { "Server" };
    if profile.kind == ProviderKind::OpenAi && profile.api_key.is_none() {
        return Err(ProviderError::MissingKey("OpenAI"));
    }
    let variants = effort_variants(profile);
    let learned_key = format!("openai:{}:{}", profile.base_url, profile.model);
    let mut variant = providers.learned_variant(&learned_key).min(variants.len() - 1);
    loop {
        let mut body = json!({
            "model": profile.model,
            "messages": [
                {"role": "system", "content": request.system},
                {"role": "user", "content": request.user},
            ],
            "stream": true,
        });
        if profile.kind == ProviderKind::OpenAi {
            body["max_completion_tokens"] = json!(request.max_tokens);
        } else {
            body["max_tokens"] = json!(request.max_tokens);
        }
        if let Some(effort) = variants[variant] {
            body["reasoning_effort"] = json!(effort);
        }
        merge_extra(&mut body, &profile.extra_body);
        let mut call = providers.http().post(profile.url("/chat/completions")).json(&body);
        if let Some(key) = profile.api_key.as_deref() {
            call = call.bearer_auth(key);
        }
        let response = call.send().await?;
        if !response.status().is_success() {
            let error = http_error(name, &profile.model, response).await;
            if variant + 1 < variants.len() && rejects_option(&error, &["reasoning", "effort"]) {
                variant += 1;
                providers.learn_variant(&learned_key, variant);
                continue;
            }
            return Err(error);
        }
        return read_stream(response, on_delta).await;
    }
}

async fn read_stream(
    response: reqwest::Response,
    on_delta: &mut (dyn FnMut(&str) + Send),
) -> Result<String, ProviderError> {
    let mut parser = SseParser::default();
    let mut output = String::new();
    let mut finish_reason: Option<String> = None;
    let mut bytes = response.bytes_stream();
    while let Some(chunk) = bytes.next().await {
        for event in parser.push(&chunk?) {
            if event.data == "[DONE]" {
                return finish(finish_reason.as_deref(), true, output);
            }
            let Ok(value) = serde_json::from_str::<Value>(&event.data) else { continue };
            if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
                return Err(ProviderError::Protocol(message.to_string()));
            }
            if let Some(text) = value.pointer("/choices/0/delta/content").and_then(Value::as_str) {
                output.push_str(text);
                on_delta(text);
            }
            if let Some(reason) = value.pointer("/choices/0/finish_reason").and_then(Value::as_str) {
                finish_reason = Some(reason.to_string());
            }
        }
    }
    finish(finish_reason.as_deref(), false, output)
}

/// A response counts as complete when the server sent `[DONE]` or a finish reason, and the
/// reason is a normal stop rather than a length limit or a content filter.
fn finish(reason: Option<&str>, done: bool, output: String) -> Result<String, ProviderError> {
    match reason {
        Some("length") => Err(ProviderError::Truncated),
        Some("content_filter") => Err(ProviderError::Declined),
        Some(_) => Ok(output),
        None if done => Ok(output),
        None => Err(ProviderError::Incomplete),
    }
}

pub(super) async fn list_models(
    providers: &Providers,
    profile: &ProviderProfile,
) -> Result<Vec<String>, ProviderError> {
    let mut call = providers.http().get(profile.url("/models"));
    if let Some(key) = profile.api_key.as_deref() {
        call = call.bearer_auth(key);
    }
    let response = call.send().await?;
    if !response.status().is_success() {
        return Err(http_error("OpenAI", &profile.model, response).await);
    }
    let value: Value = response.json().await?;
    let skip =
        ["embedding", "whisper", "tts", "dall-e", "image", "audio", "realtime", "moderation", "transcribe", "search"];
    Ok(value["data"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|m| m["id"].as_str())
                .filter(|id| profile.kind != ProviderKind::OpenAi || !skip.iter().any(|s| id.contains(s)))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default())
}
