use super::{ChatRequest, ProviderError, ProviderProfile, Providers, http_error, merge_extra, rejects_option};
use crate::sse::SseParser;
use futures_util::StreamExt;
use serde_json::{Value, json};

const NAME: &str = "Anthropic";
const VERSION: &str = "2023-06-01";

/// Latency settings, most aggressive first. Thinking can be disabled on Haiku 5.5, Sonnet 5 and
/// Opus 5; newer Opus/Sonnet models only accept a lower effort; 4.x models need neither.
fn latency_variants(model: &str) -> Vec<Value> {
    let disable = json!({"thinking": {"type": "disabled"}, "output_config": {"effort": "low"}});
    let low = json!({"output_config": {"effort": "low"}});
    let none = json!({});
    let can_disable = model.starts_with("claude-haiku-5") || model == "claude-sonnet-5" || model == "claude-opus-5";
    let effort_only = ["claude-opus-5", "claude-sonnet-5", "claude-fable", "claude-mythos"]
        .iter()
        .any(|prefix| model.starts_with(prefix));
    if can_disable {
        vec![disable, low, none]
    } else if effort_only {
        vec![low, none]
    } else {
        vec![none]
    }
}

pub(super) async fn stream(
    providers: &Providers,
    profile: &ProviderProfile,
    request: &ChatRequest,
    on_delta: &mut (dyn FnMut(&str) + Send),
) -> Result<String, ProviderError> {
    let key = profile.api_key.as_deref().ok_or(ProviderError::MissingKey(NAME))?;
    let variants = latency_variants(&profile.model);
    let learned_key = format!("anthropic:{}", profile.model);
    let mut variant = providers.learned_variant(&learned_key).min(variants.len() - 1);
    loop {
        let mut body = json!({
            "model": profile.model,
            "max_tokens": request.max_tokens,
            "system": request.system,
            "messages": [{"role": "user", "content": request.user}],
            "stream": true,
        });
        merge_extra(&mut body, &Some(variants[variant].clone()));
        merge_extra(&mut body, &profile.extra_body);
        let response = providers
            .http()
            .post(profile.url("/v1/messages"))
            .header("x-api-key", key)
            .header("anthropic-version", VERSION)
            .json(&body)
            .send()
            .await?;
        if !response.status().is_success() {
            let error = http_error(NAME, &profile.model, response).await;
            if variant + 1 < variants.len() && rejects_option(&error, &["thinking", "effort", "output_config"]) {
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
    let mut stop_reason = None;
    let mut bytes = response.bytes_stream();
    while let Some(chunk) = bytes.next().await {
        for event in parser.push(&chunk?) {
            let Ok(value) = serde_json::from_str::<Value>(&event.data) else { continue };
            match value["type"].as_str() {
                Some("content_block_delta") if value["delta"]["type"] == "text_delta" => {
                    if let Some(text) = value["delta"]["text"].as_str() {
                        output.push_str(text);
                        on_delta(text);
                    }
                }
                Some("message_delta") => {
                    if let Some(reason) = value["delta"]["stop_reason"].as_str() {
                        stop_reason = Some(reason.to_string());
                    }
                }
                Some("error") => {
                    let message = value["error"]["message"].as_str().unwrap_or("stream error");
                    if value["error"]["type"] == "overloaded_error" {
                        return Err(ProviderError::RateLimited);
                    }
                    return Err(ProviderError::Protocol(message.to_string()));
                }
                Some("message_stop") => {
                    return match stop_reason.as_deref() {
                        Some("max_tokens") => Err(ProviderError::Truncated),
                        Some("refusal") => Err(ProviderError::Declined),
                        _ => Ok(output),
                    };
                }
                _ => {}
            }
        }
    }
    Err(ProviderError::Incomplete)
}

pub(super) async fn list_models(
    providers: &Providers,
    profile: &ProviderProfile,
) -> Result<Vec<String>, ProviderError> {
    let key = profile.api_key.as_deref().ok_or(ProviderError::MissingKey(NAME))?;
    let response = providers
        .http()
        .get(profile.url("/v1/models?limit=100"))
        .header("x-api-key", key)
        .header("anthropic-version", VERSION)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(http_error(NAME, &profile.model, response).await);
    }
    let value: Value = response.json().await?;
    Ok(value["data"]
        .as_array()
        .map(|items| items.iter().filter_map(|m| m["id"].as_str().map(str::to_string)).collect())
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::latency_variants;

    #[test]
    fn variants_match_model_rules() {
        assert_eq!(latency_variants("claude-haiku-5-5").len(), 3);
        assert_eq!(latency_variants("claude-haiku-5-5")[0]["thinking"]["type"], "disabled");
        assert!(latency_variants("claude-opus-5-5")[0].get("thinking").is_none());
        assert_eq!(latency_variants("claude-haiku-4-5"), vec![serde_json::json!({})]);
    }
}
