//! Apple Intelligence provider tests against a mock helper that speaks the helper's JSON-lines
//! protocol, so they run on any Mac (and in CI) without the on-device model.

use std::path::{Path, PathBuf};
use std::time::Duration;
use supereditr_core::provider::{ChatRequest, ProviderKind, ProviderProfile, Providers};

/// Writes a mock helper script. `hello` is its startup line; every received line is logged.
fn mock_helper(name: &str, hello: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("supereditr-apple-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("received.log");
    let script = dir.join("supereditr-fm");
    let body = format!(
        r#"#!/bin/sh
printf '%s\n' '{hello}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"type":"status"'*) printf '%s\n' "$(cat '{status}' 2>/dev/null || printf '%s' '{hello}')" | sed "s/\"type\":\"hello\"/\"type\":\"status\",\"id\":$id/" ;;
    *'"type":"request"'*)
      case "$line" in
        *TOO_LONG*) printf '{{"type":"error","id":%s,"code":"contextWindow","message":"x"}}\n' "$id" ;;
        *SLOW*) ;;
        *CRASH*) exit 1 ;;
        *) printf '{{"type":"delta","id":%s,"text":"This is "}}\n' "$id"
           printf '{{"type":"delta","id":%s,"text":"teh draft."}}\n' "$id"
           printf '{{"type":"done","id":%s,"text":"This is the draft."}}\n' "$id" ;;
      esac ;;
  esac
done
"#,
        log = log.display(),
        status = dir.join("status.json").display()
    );
    std::fs::write(&script, body).unwrap();
    std::process::Command::new("chmod").arg("+x").arg(&script).status().unwrap();
    (script, log)
}

fn profile(helper: &Path) -> ProviderProfile {
    let mut p = ProviderProfile::new(ProviderKind::AppleIntelligence);
    p.base_url = helper.display().to_string();
    p
}

fn request(text: &str) -> ChatRequest {
    ChatRequest { system: "Fix it.".into(), user: format!("<text>{text}</text>"), max_tokens: 50_000 }
}

const AVAILABLE: &str = r#"{"type":"hello","available":true}"#;

#[tokio::test]
async fn streams_deltas_and_trusts_the_final_text() {
    let (helper, log) = mock_helper("stream", AVAILABLE);
    let providers = Providers::new();
    let mut deltas = Vec::new();
    let text = providers
        .stream(&profile(&helper), &request("This are teh draft."), &mut |d: &str| deltas.push(d.to_string()))
        .await
        .unwrap();
    assert_eq!(deltas, vec!["This is ", "teh draft."]);
    assert_eq!(text, "This is the draft.", "the done message's text wins over streamed fragments");
    let sent = std::fs::read_to_string(log).unwrap();
    assert!(sent.contains(r#""maxTokens":2000"#), "response length is capped for the small context window: {sent}");
}

#[tokio::test]
async fn reuses_one_resident_helper() {
    let (helper, log) = mock_helper("resident", AVAILABLE);
    let providers = Providers::new();
    for _ in 0..3 {
        providers.stream(&profile(&helper), &request("x"), &mut |_: &str| {}).await.unwrap();
    }
    let sent = std::fs::read_to_string(log).unwrap();
    let ids: Vec<&str> = sent.lines().filter_map(|l| l.split(r#""id":"#).nth(1)).map(|r| &r[..1]).collect();
    assert_eq!(ids, vec!["1", "2", "3"], "one process served all three requests: {sent}");
}

#[tokio::test]
async fn maps_helper_errors_to_plain_messages() {
    let (helper, _) = mock_helper("errors", AVAILABLE);
    let error = Providers::new().stream(&profile(&helper), &request("TOO_LONG"), &mut |_: &str| {}).await.unwrap_err();
    assert!(error.to_string().contains("too long for Apple"), "{error}");
}

#[tokio::test]
async fn reports_why_the_model_is_unavailable() {
    let (helper, _) =
        mock_helper("unavailable", r#"{"type":"hello","available":false,"reason":"appleIntelligenceNotEnabled"}"#);
    let providers = Providers::new();
    let status = providers.apple_status(&profile(&helper)).await;
    assert!(!status.available);
    let error = providers.list_models(&profile(&helper)).await.unwrap_err();
    assert!(error.to_string().contains("Turn on Apple Intelligence"), "{error}");
}

#[tokio::test]
async fn notices_when_apple_intelligence_is_turned_on_after_launch() {
    let (helper, _) =
        mock_helper("turned-on", r#"{"type":"hello","available":false,"reason":"appleIntelligenceNotEnabled"}"#);
    let providers = Providers::new();
    assert!(!providers.apple_status(&profile(&helper)).await.available);
    std::fs::write(helper.with_file_name("status.json"), AVAILABLE).unwrap();
    assert!(providers.apple_status(&profile(&helper)).await.available, "status is asked again, not cached");
}

#[tokio::test]
async fn abandoning_a_request_cancels_it_in_the_helper() {
    let (helper, log) = mock_helper("cancel", AVAILABLE);
    let providers = Providers::new();
    let p = profile(&helper);
    let attempt =
        tokio::time::timeout(Duration::from_millis(300), providers.stream(&p, &request("SLOW"), &mut |_: &str| {}))
            .await;
    assert!(attempt.is_err(), "the slow request is still running when abandoned");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let sent = std::fs::read_to_string(log).unwrap();
    assert!(sent.contains(r#""type":"cancel""#), "{sent}");
}

#[tokio::test]
async fn restarts_the_helper_after_it_exits() {
    let (helper, _) = mock_helper("restart", AVAILABLE);
    let providers = Providers::new();
    let p = profile(&helper);
    let error = providers.stream(&p, &request("CRASH"), &mut |_: &str| {}).await.unwrap_err();
    assert!(error.to_string().contains("stopped unexpectedly"), "{error}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let text = providers.stream(&p, &request("again"), &mut |_: &str| {}).await.unwrap();
    assert_eq!(text, "This is the draft.");
}

#[tokio::test]
async fn missing_helper_is_explained() {
    let mut p = ProviderProfile::new(ProviderKind::AppleIntelligence);
    p.base_url = "/nonexistent/supereditr-fm".into();
    let error = Providers::new().stream(&p, &request("x"), &mut |_: &str| {}).await.unwrap_err();
    assert!(error.to_string().contains("Apple Intelligence helper"), "{error}");
}
