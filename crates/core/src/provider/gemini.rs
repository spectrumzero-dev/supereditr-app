use super::{ChatRequest, ProviderError, ProviderProfile, Providers, http_error, merge_extra, rejects_option};
use crate::sse::SseParser;
use futures_util::StreamExt;
use serde_json::{Value, json};

const NAME: &str = "Gemini";

pub(super) async fn stream(
    providers: &Providers,
    profile: &ProviderProfile,
    request: &ChatRequest,
    on_delta: &mut (dyn FnMut(&str) + Send),
) -> Result<String, ProviderError> {
    let key = profile.api_key.as_deref().ok_or(ProviderError::MissingKey(NAME))?;
    // Thinking off where the model allows it; otherwise the model's own default.
    // Gemini 2.5 takes a token budget; Gemini 3 and later take a thinking level.
    let variants = [
        Some(json!({"thinkingBudget": 0})),
        Some(json!({"thinkingLevel": "minimal"})),
        Some(json!({"thinkingLevel": "low"})),
        None,
    ];
    let learned_key = format!("gemini:{}", profile.model);
    let mut variant = providers.learned_variant(&learned_key).min(variants.len() - 1);
    let model = profile.model.trim_start_matches("models/");
    loop {
        let mut generation = json!({"maxOutputTokens": request.max_tokens});
        if let Some(thinking) = &variants[variant] {
            generation["thinkingConfig"] = thinking.clone();
        }
        let mut body = json!({
            "systemInstruction": {"parts": [{"text": request.system}]},
            "contents": [{"role": "user", "parts": [{"text": request.user}]}],
            "generationConfig": generation,
        });
        merge_extra(&mut body, &profile.extra_body);
        let response = providers
            .http()
            .post(profile.url(&format!("/v1beta/models/{model}:streamGenerateContent?alt=sse")))
            .header("x-goog-api-key", key)
            .json(&body)
            .send()
            .await?;
        if !response.status().is_success() {
            let error = http_error(NAME, &profile.model, response).await;
            if variant + 1 < variants.len() && rejects_option(&error, &["thinking", "budget", "level"]) {
                variant += 1;
                providers.learn_variant(&learned_key, variant);
                continue;
            }
            return Err(error);
        }
        let mut parser = SseParser::default();
        let mut output = String::new();
        let mut finish_reason: Option<String> = None;
        let mut bytes = response.bytes_stream();
        while let Some(chunk) = bytes.next().await {
            for event in parser.push(&chunk?) {
                let Ok(value) = serde_json::from_str::<Value>(&event.data) else { continue };
                if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
                    return Err(ProviderError::Protocol(message.to_string()));
                }
                if let Some(reason) = value.pointer("/candidates/0/finishReason").and_then(Value::as_str) {
                    finish_reason = Some(reason.to_string());
                }
                let parts = value.pointer("/candidates/0/content/parts").and_then(Value::as_array);
                for part in parts.into_iter().flatten() {
                    if part["thought"].as_bool() == Some(true) {
                        continue;
                    }
                    if let Some(text) = part["text"].as_str() {
                        output.push_str(text);
                        on_delta(text);
                    }
                }
            }
        }
        return match finish_reason.as_deref() {
            Some("STOP") => Ok(output),
            Some("MAX_TOKENS") => Err(ProviderError::Truncated),
            Some(_) => Err(ProviderError::Declined),
            None => Err(ProviderError::Incomplete),
        };
    }
}

pub(super) async fn list_models(
    providers: &Providers,
    profile: &ProviderProfile,
) -> Result<Vec<String>, ProviderError> {
    let key = profile.api_key.as_deref().ok_or(ProviderError::MissingKey(NAME))?;
    let response =
        providers.http().get(profile.url("/v1beta/models?pageSize=200")).header("x-goog-api-key", key).send().await?;
    if !response.status().is_success() {
        return Err(http_error(NAME, &profile.model, response).await);
    }
    let value: Value = response.json().await?;
    Ok(value["models"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|m| {
                    m["supportedGenerationMethods"]
                        .as_array()
                        .is_some_and(|methods| methods.iter().any(|x| x == "generateContent"))
                })
                .filter_map(|m| m["name"].as_str())
                .map(|name| name.trim_start_matches("models/").to_string())
                .filter(|name| name.starts_with("gemini") && !name.contains("image") && !name.contains("tts"))
                .collect()
        })
        .unwrap_or_default())
}
