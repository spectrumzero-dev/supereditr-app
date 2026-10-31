//! End-to-end provider tests against a disposable local HTTP server that replays each
//! provider's streaming wire format. No network access or real keys are involved.

use std::sync::{Arc, Mutex};
use supereditr_core::provider::{ChatRequest, ProviderKind, ProviderProfile, Providers};
use supereditr_core::{Engine, Event, Job};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serves `responses` in order, one per connection, and records each request.
async fn mock_server(responses: Vec<(u16, &'static str, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        for (status, content_type, body) in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                let n = socket.read(&mut buf).await.unwrap();
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(header_end) = text.find("\r\n\r\n") {
                    let length = text
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + 4 + length {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            log.lock().unwrap().push(String::from_utf8_lossy(&request).to_string());
            let reason = if status == 200 { "OK" } else { "Bad Request" };
            let head = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(body.as_bytes()).await.unwrap();
            socket.shutdown().await.ok();
        }
    });
    (address, seen)
}

fn request() -> ChatRequest {
    ChatRequest { system: "sys".into(), user: "<text>This are teh draft.</text>".into(), max_tokens: 100 }
}

fn profile(kind: ProviderKind, base_url: &str, model: &str) -> ProviderProfile {
    let mut p = ProviderProfile::new(kind);
    p.base_url = base_url.to_string();
    p.model = model.to_string();
    p.api_key = Some("test-key".into());
    p
}

async fn collect(providers: &Providers, profile: &ProviderProfile) -> (String, Vec<String>) {
    let mut deltas = Vec::new();
    let text = providers.stream(profile, &request(), &mut |d: &str| deltas.push(d.to_string())).await.unwrap();
    (text, deltas)
}

#[tokio::test]
async fn anthropic_streams_text_and_skips_thinking() {
    let body = [
        r#"event: message_start
data: {"type":"message_start","message":{}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"This is "}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"the draft."}}"#,
        r#"event: message_stop
data: {"type":"message_stop"}"#,
    ]
    .join("\n\n")
        + "\n\n";
    let (url, seen) = mock_server(vec![(200, "text/event-stream", body)]).await;
    let (text, deltas) = collect(&Providers::new(), &profile(ProviderKind::Anthropic, &url, "claude-haiku-5-5")).await;
    assert_eq!(text, "This is the draft.");
    assert_eq!(deltas.len(), 2);
    let sent = &seen.lock().unwrap()[0];
    assert!(sent.starts_with("POST /v1/messages"));
    assert!(sent.contains("x-api-key: test-key"));
    assert!(sent.contains(r#""thinking":{"type":"disabled"}"#));
}

#[tokio::test]
async fn anthropic_retries_without_rejected_thinking_option() {
    let error = r#"{"type":"error","error":{"type":"invalid_request_error","message":"thinking.type: disabled is not supported"}}"#;
    let ok = "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\ndata: {\"type\":\"message_stop\"}\n\n";
    let (url, seen) =
        mock_server(vec![(400, "application/json", error.into()), (200, "text/event-stream", ok.into())]).await;
    let providers = Providers::new();
    let (text, _) = collect(&providers, &profile(ProviderKind::Anthropic, &url, "claude-haiku-5-5")).await;
    assert_eq!(text, "ok");
    let log = seen.lock().unwrap();
    assert!(log[0].contains("disabled"));
    assert!(!log[1].contains("disabled"));
}

#[tokio::test]
async fn openai_streams_chat_completions() {
    let body = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n\
                data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n\
                data: {\"choices\":[{\"delta\":{\"content\":\", world\"}}]}\n\n\
                data: [DONE]\n\n";
    let (url, seen) = mock_server(vec![(200, "text/event-stream", body.into())]).await;
    let (text, _) = collect(&Providers::new(), &profile(ProviderKind::OpenAi, &url, "gpt-4.1-mini")).await;
    assert_eq!(text, "Hello, world");
    let sent = &seen.lock().unwrap()[0];
    assert!(sent.starts_with("POST /chat/completions"));
    assert!(sent.to_lowercase().contains("authorization: bearer test-key"));
    assert!(sent.contains("max_completion_tokens"));
    assert!(!sent.contains("reasoning_effort"));
}

#[tokio::test]
async fn openai_reasoning_models_fall_back_through_effort_levels() {
    let reject = r#"{"error":{"message":"Unsupported value: 'reasoning_effort' does not support 'none'"}}"#;
    let ok = "data: {\"choices\":[{\"delta\":{\"content\":\"done\"}}]}\n\ndata: [DONE]\n\n";
    let (url, seen) =
        mock_server(vec![(400, "application/json", reject.into()), (200, "text/event-stream", ok.into())]).await;
    let (text, _) = collect(&Providers::new(), &profile(ProviderKind::OpenAi, &url, "gpt-5-mini")).await;
    assert_eq!(text, "done");
    let log = seen.lock().unwrap();
    assert!(log[0].contains(r#""reasoning_effort":"none""#));
    assert!(log[1].contains(r#""reasoning_effort":"minimal""#));
}

#[tokio::test]
async fn gemini_streams_and_ignores_thought_parts() {
    let body = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"plan\",\"thought\":true}]}}]}\n\n\
                data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Bonjour\"}]}}]}\n\n\
                data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\" à tous\"}]},\"finishReason\":\"STOP\"}]}\n\n";
    let (url, seen) = mock_server(vec![(200, "text/event-stream", body.into())]).await;
    let (text, _) = collect(&Providers::new(), &profile(ProviderKind::Gemini, &url, "gemini-2.5-flash-lite")).await;
    assert_eq!(text, "Bonjour à tous");
    let sent = &seen.lock().unwrap()[0];
    assert!(sent.starts_with("POST /v1beta/models/gemini-2.5-flash-lite:streamGenerateContent?alt=sse"));
    assert!(sent.contains("x-goog-api-key: test-key"));
}

#[tokio::test]
async fn ollama_streams_ndjson_and_keeps_model_loaded() {
    let body = "{\"message\":{\"content\":\"Fixed\"},\"done\":false}\n{\"message\":{\"content\":\" text\"},\"done\":false}\n{\"done\":true}\n";
    let (url, seen) = mock_server(vec![(200, "application/x-ndjson", body.into())]).await;
    let (text, _) = collect(&Providers::new(), &profile(ProviderKind::Ollama, &url, "gemma3:4b")).await;
    assert_eq!(text, "Fixed text");
    let sent = &seen.lock().unwrap()[0];
    assert!(sent.contains(r#""keep_alive":"30m""#));
    assert!(sent.contains(r#""think":false"#));
}

#[tokio::test]
async fn auth_errors_are_typed() {
    let (url, _) = mock_server(vec![(401, "application/json", r#"{"error":{"message":"bad key"}}"#.into())]).await;
    let error = Providers::new()
        .stream(&profile(ProviderKind::OpenAi, &url, "gpt-4.1-mini"), &request(), &mut |_: &str| {})
        .await
        .unwrap_err();
    assert!(error.to_string().contains("rejected the API key"), "{error}");
}

#[tokio::test]
async fn engine_leaves_the_offline_checker_out_while_an_ai_answers() {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"<text>This is \"}}]}\n\n\
                data: {\"choices\":[{\"delta\":{\"content\":\"the draft.</text>\"}}]}\n\n\
                data: [DONE]\n\n";
    let (url, _) = mock_server(vec![(200, "text/event-stream", body.into())]).await;
    let engine = Engine::new("American English");
    let config = supereditr_core::Config::default();
    let job = Job {
        text: "This are teh draft.\n".into(),
        preset: config.default_preset().clone(),
        steer: None,
        profile: Some(profile(ProviderKind::OpenAiCompatible, &url, "local-model")),
        style_notes: String::new(),
        dialect: "American English".into(),
        words: Vec::new(),
        code: Vec::new(),
        local_checks: true,
        builtin_ai: false,
    };
    let mut events = Vec::new();
    engine.run(job, &mut |e| events.push(e)).await;
    assert!(!events.iter().any(|e| matches!(e, Event::Local { .. })), "no offline guesses while the AI answers");
    let Some(Event::Done(outcome)) = events.last() else { panic!("{events:?}") };
    assert_eq!(outcome.text, "This is the draft.\n", "tags stripped, trailing newline restored");
    assert_eq!(outcome.view.change_count, 2);
}

/// The offline checker's final pass over the AI's revision keeps a name the dictionary also lists
/// as an acronym ("Sam", "SAM").
#[tokio::test]
async fn engine_final_pass_keeps_names() {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"<text>Hi Sam, there are a few typos.</text>\"}}]}\n\n\
                data: [DONE]\n\n";
    let (url, _) = mock_server(vec![(200, "text/event-stream", body.into())]).await;
    let engine = Engine::new("American English");
    let config = supereditr_core::Config::default();
    let job = Job {
        text: "Hi Sam, their are a few typos.".into(),
        preset: config.default_preset().clone(),
        steer: None,
        profile: Some(profile(ProviderKind::OpenAiCompatible, &url, "local-model")),
        style_notes: String::new(),
        dialect: "American English".into(),
        words: Vec::new(),
        code: Vec::new(),
        local_checks: true,
        builtin_ai: false,
    };
    let mut events = Vec::new();
    engine.run(job, &mut |e| events.push(e)).await;
    let Some(Event::Done(outcome)) = events.last() else { panic!("{events:?}") };
    assert_eq!(outcome.text, "Hi Sam, there are a few typos.");
    assert_eq!(outcome.view.change_count, 1);
}

#[tokio::test]
async fn engine_leaves_code_as_written() {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"<text>Should I update? `Wrangler 4.150.0` \"}}]}\n\n\
                data: {\"choices\":[{\"delta\":{\"content\":\"now.\\n```\\nNpm i\\n```</text>\"}}]}\n\n\
                data: [DONE]\n\n";
    let (url, _) = mock_server(vec![(200, "text/event-stream", body.into())]).await;
    let engine = Engine::new("American English");
    let config = supereditr_core::Config::default();
    let job = Job {
        text: "should i update? `wrangler 4.150.0` now\n```\nnpm i\n```".into(),
        preset: config.default_preset().clone(),
        steer: None,
        profile: Some(profile(ProviderKind::OpenAiCompatible, &url, "local-model")),
        style_notes: String::new(),
        dialect: "American English".into(),
        words: Vec::new(),
        code: Vec::new(),
        local_checks: true,
        builtin_ai: false,
    };
    let mut events = Vec::new();
    engine.run(job, &mut |e| events.push(e)).await;
    let Some(Event::Done(outcome)) = events.last() else { panic!("{events:?}") };
    assert_eq!(outcome.text, "Should I update? `wrangler 4.150.0` now.\n```\nnpm i\n```");
    assert_eq!(outcome.view.change_count, 3);
}

#[tokio::test]
async fn engine_leaves_code_the_app_formats_as_written() {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"<text>Should I update? Wrangler 4.150.0</text>\"}}]}\n\n\
                data: [DONE]\n\n";
    let (url, _) = mock_server(vec![(200, "text/event-stream", body.into())]).await;
    let engine = Engine::new("American English");
    let config = supereditr_core::Config::default();
    let job = Job {
        text: "should i update? wrangler 4.150.0".into(),
        preset: config.default_preset().clone(),
        steer: None,
        profile: Some(profile(ProviderKind::OpenAiCompatible, &url, "local-model")),
        style_notes: String::new(),
        dialect: "American English".into(),
        words: Vec::new(),
        code: vec![(17, 33)],
        local_checks: true,
        builtin_ai: false,
    };
    let mut events = Vec::new();
    engine.run(job, &mut |e| events.push(e)).await;
    let Some(Event::Done(outcome)) = events.last() else { panic!("{events:?}") };
    assert_eq!(outcome.text, "Should I update? wrangler 4.150.0");
}

#[tokio::test]
async fn engine_falls_back_to_the_offline_checker_when_the_ai_fails() {
    let (url, _) = mock_server(vec![(500, "application/json", "{}".into())]).await;
    let engine = Engine::new("American English");
    let config = supereditr_core::Config::default();
    let job = Job {
        text: "This is an test.".into(),
        preset: config.default_preset().clone(),
        steer: None,
        profile: Some(profile(ProviderKind::OpenAiCompatible, &url, "local-model")),
        style_notes: String::new(),
        dialect: "American English".into(),
        words: Vec::new(),
        code: Vec::new(),
        local_checks: true,
        builtin_ai: false,
    };
    let mut events = Vec::new();
    engine.run(job, &mut |e| events.push(e)).await;
    assert!(matches!(events.first(), Some(Event::Local { .. })), "{events:?}");
    assert!(matches!(events.last(), Some(Event::Failed { local_only: true, .. })), "{events:?}");
}

#[tokio::test]
async fn engine_works_offline_without_a_provider() {
    let engine = Engine::new("American English");
    let config = supereditr_core::Config::default();
    let job = Job {
        text: "This is an test.".into(),
        preset: config.default_preset().clone(),
        steer: None,
        profile: None,
        style_notes: String::new(),
        dialect: "American English".into(),
        words: Vec::new(),
        code: Vec::new(),
        local_checks: true,
        builtin_ai: false,
    };
    let mut events = Vec::new();
    engine.run(job, &mut |e| events.push(e)).await;
    let Some(Event::Done(outcome)) = events.last() else { panic!("{events:?}") };
    assert_eq!(outcome.text, "This is a test.");
}

/// A selection or an instruction holding a secret never reaches a provider or the offline checker.
#[tokio::test]
async fn engine_never_sends_a_secret() {
    // Assembled from pieces so the source holds no complete token for scanners to flag.
    let token = format!("{}{}", "ghp_", "Ab3dE5gH7jK9mN1pQ3sT5vW7yZ9bC1dF2hJ4");
    let config = supereditr_core::Config::default();
    for (text, steer) in
        [(format!("Deploy with {token} today."), None), ("Their here.".to_string(), Some(token.clone()))]
    {
        let (url, seen) = mock_server(vec![(200, "text/event-stream", String::new())]).await;
        let job = Job {
            text,
            preset: config.default_preset().clone(),
            steer,
            profile: Some(profile(ProviderKind::OpenAiCompatible, &url, "local-model")),
            style_notes: String::new(),
            dialect: "American English".into(),
            words: Vec::new(),
            code: Vec::new(),
            local_checks: true,
            builtin_ai: true,
        };
        let mut events = Vec::new();
        Engine::new("American English").run(job, &mut |e| events.push(e)).await;
        let [Event::Failed { message, local_only: false }] = events.as_slice() else { panic!("{events:?}") };
        assert!(message.contains("a GitHub token") && !message.contains(&token), "{message}");
        assert!(seen.lock().unwrap().is_empty(), "the provider was called");
    }
}

async fn stream_error(kind: ProviderKind, model: &str, content_type: &'static str, body: &str) -> String {
    let (url, _) = mock_server(vec![(200, content_type, body.to_string())]).await;
    Providers::new().stream(&profile(kind, &url, model), &request(), &mut |_: &str| {}).await.unwrap_err().to_string()
}

#[tokio::test]
async fn length_limited_responses_are_not_treated_as_finished() {
    let anthropic = "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"This is\"}}\n\n\
                     data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n\
                     data: {\"type\":\"message_stop\"}\n\n";
    let openai =
        "data: {\"choices\":[{\"delta\":{\"content\":\"This is\"},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n";
    let gemini = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"This is\"}]},\"finishReason\":\"MAX_TOKENS\"}]}\n\n";
    let ollama = "{\"message\":{\"content\":\"This is\"},\"done\":true,\"done_reason\":\"length\"}\n";
    for error in [
        stream_error(ProviderKind::Anthropic, "claude-haiku-4-5", "text/event-stream", anthropic).await,
        stream_error(ProviderKind::OpenAi, "gpt-4.1-mini", "text/event-stream", openai).await,
        stream_error(ProviderKind::Gemini, "gemini-2.5-flash-lite", "text/event-stream", gemini).await,
        stream_error(ProviderKind::Ollama, "gemma3:4b", "application/x-ndjson", ollama).await,
    ] {
        assert!(error.contains("length limit"), "{error}");
    }
}

#[tokio::test]
async fn streams_that_end_early_are_errors() {
    let anthropic =
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"This is\"}}\n\n";
    let openai = "data: {\"choices\":[{\"delta\":{\"content\":\"This is\"}}]}\n\n";
    let gemini = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"This is\"}]}}]}\n\n";
    let ollama = "{\"message\":{\"content\":\"This is\"},\"done\":false}\n";
    for error in [
        stream_error(ProviderKind::Anthropic, "claude-haiku-4-5", "text/event-stream", anthropic).await,
        stream_error(ProviderKind::OpenAi, "gpt-4.1-mini", "text/event-stream", openai).await,
        stream_error(ProviderKind::Gemini, "gemini-2.5-flash-lite", "text/event-stream", gemini).await,
        stream_error(ProviderKind::Ollama, "gemma3:4b", "application/x-ndjson", ollama).await,
    ] {
        assert!(error.contains("ended before the model finished"), "{error}");
    }
}

#[tokio::test]
async fn refusals_and_filters_are_declines() {
    let anthropic = "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"refusal\"}}\n\ndata: {\"type\":\"message_stop\"}\n\n";
    let openai = "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"content_filter\"}]}\n\ndata: [DONE]\n\n";
    let gemini = "data: {\"candidates\":[{\"finishReason\":\"SAFETY\"}]}\n\n";
    for error in [
        stream_error(ProviderKind::Anthropic, "claude-haiku-4-5", "text/event-stream", anthropic).await,
        stream_error(ProviderKind::OpenAi, "gpt-4.1-mini", "text/event-stream", openai).await,
        stream_error(ProviderKind::Gemini, "gemini-2.5-flash-lite", "text/event-stream", gemini).await,
    ] {
        assert!(error.contains("declined"), "{error}");
    }
}

#[tokio::test]
async fn engine_rejects_an_empty_revision() {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"  \"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let (url, _) = mock_server(vec![(200, "text/event-stream", body.into())]).await;
    let config = supereditr_core::Config::default();
    let job = Job {
        text: "Keep this text.".into(),
        preset: config.default_preset().clone(),
        steer: None,
        profile: Some(profile(ProviderKind::OpenAiCompatible, &url, "local-model")),
        style_notes: String::new(),
        dialect: "American English".into(),
        words: Vec::new(),
        code: Vec::new(),
        local_checks: false,
        builtin_ai: false,
    };
    let mut events = Vec::new();
    Engine::new("American English").run(job, &mut |e| events.push(e)).await;
    assert!(matches!(events.last(), Some(Event::Failed { .. })), "{events:?}");
}

#[tokio::test]
async fn engine_rejects_a_wrapped_empty_revision() {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"<text></text>\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let (url, _) = mock_server(vec![(200, "text/event-stream", body.into())]).await;
    let config = supereditr_core::Config::default();
    let job = Job {
        text: "Keep this text.".into(),
        preset: config.default_preset().clone(),
        steer: None,
        profile: Some(profile(ProviderKind::OpenAiCompatible, &url, "local-model")),
        style_notes: String::new(),
        dialect: "American English".into(),
        words: Vec::new(),
        code: Vec::new(),
        local_checks: false,
        builtin_ai: false,
    };
    let mut events = Vec::new();
    Engine::new("American English").run(job, &mut |e| events.push(e)).await;
    assert!(matches!(events.last(), Some(Event::Failed { .. })), "{events:?}");
}

/// A stand-in for the `claude` binary that prints the given stream-json lines and exit code.
fn fake_claude(name: &str, lines: &[&str], exit_code: i32) -> ProviderProfile {
    let dir = std::env::temp_dir().join(format!("supereditr-fake-claude-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("claude");
    let mut body = String::from("#!/bin/sh\ncat > /dev/null\n");
    for line in lines {
        body.push_str(&format!("printf '%s\\n' '{line}'\n"));
    }
    body.push_str(&format!("exit {exit_code}\n"));
    std::fs::write(&script, body).unwrap();
    std::process::Command::new("chmod").arg("+x").arg(&script).status().unwrap();
    let mut p = ProviderProfile::new(ProviderKind::ClaudeCli);
    p.base_url = script.display().to_string();
    p
}

const CLI_DELTA: &str = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"This is teh"}}}"#;

#[tokio::test]
async fn claude_cli_needs_a_final_result() {
    let providers = Providers::new();
    let partial = fake_claude("partial", &[CLI_DELTA], 0);
    let error = providers.stream(&partial, &request(), &mut |_: &str| {}).await.unwrap_err();
    assert!(error.to_string().contains("ended before the model finished"), "{error}");

    let failed = fake_claude("failed", &[CLI_DELTA], 1);
    assert!(providers.stream(&failed, &request(), &mut |_: &str| {}).await.is_err());

    let complete =
        fake_claude("complete", &[CLI_DELTA, r#"{"type":"result","is_error":false,"result":"This is the draft."}"#], 0);
    let text = providers.stream(&complete, &request(), &mut |_: &str| {}).await.unwrap();
    assert_eq!(text, "This is the draft.", "the result event is authoritative over streamed fragments");
}

#[tokio::test]
async fn changes_that_drop_the_authors_terms_start_rejected() {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"<text>Show KPI on a quick dashboard today.</text>\"}}]}\n\n\
                data: [DONE]\n\n";
    let (url, _) = mock_server(vec![(200, "text/event-stream", body.into())]).await;
    let engine = Engine::new("American English");
    let config = supereditr_core::Config::default();
    let job = Job {
        text: "Show KPI on a glanceable dashboard tomorow.".into(),
        preset: config.default_preset().clone(),
        steer: None,
        profile: Some(profile(ProviderKind::OpenAiCompatible, &url, "local-model")),
        style_notes: String::new(),
        dialect: "American English".into(),
        words: Vec::new(),
        code: Vec::new(),
        local_checks: true,
        builtin_ai: false,
    };
    let mut events = Vec::new();
    engine.run(job, &mut |e| events.push(e)).await;
    let Some(Event::Done(outcome)) = events.last() else { panic!("{events:?}") };
    assert_eq!(outcome.kept_terms, vec!["glanceable"], "KPI survived; the typo fix isn't a kept term");
    assert_eq!(outcome.kept.len(), 1, "only the glanceable change starts rejected: {:?}", outcome.view.segments);
}

#[tokio::test]
async fn the_authors_words_reach_the_ai_and_survive_it() {
    // The model "fixes" the name and the address; Harper's pass mustn't touch them either.
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"<text>I am taking Ryan to tennis at 1234 Glenmoor Court.</text>\"}}]}\n\n\
                data: [DONE]\n\n";
    let (url, seen) = mock_server(vec![(200, "text/event-stream", body.into())]).await;
    let engine = Engine::new("American English");
    let config = supereditr_core::Config::default();
    let job = Job {
        text: "I am taking Rhyan to tennis at 1234 Glenmoor Ct.".into(),
        preset: config.default_preset().clone(),
        steer: None,
        profile: Some(profile(ProviderKind::OpenAiCompatible, &url, "local-model")),
        style_notes: String::new(),
        dialect: "American English".into(),
        words: vec!["Rhyan".into(), "1234 Glenmoor Ct".into(), "QuillPoint".into()],
        code: Vec::new(),
        local_checks: true,
        builtin_ai: false,
    };
    let mut events = Vec::new();
    engine.run(job, &mut |e| events.push(e)).await;
    let request = seen.lock().unwrap()[0].clone();
    assert!(request.contains("Keep them exactly as written: Rhyan, 1234 Glenmoor Ct"), "{request}");
    assert!(!request.contains("QuillPoint"), "only entries in the selection are sent");
    let Some(Event::Done(outcome)) = events.last() else { panic!("{events:?}") };
    assert_eq!(outcome.kept_terms, vec!["Rhyan", "1234 Glenmoor Ct"]);
    assert_eq!(outcome.kept.len(), 2, "both changes start rejected: {:?}", outcome.view.segments);
    let kept: std::collections::HashSet<usize> = outcome.kept.iter().copied().collect();
    assert_eq!(supereditr_core::diff::apply(&outcome.view, &kept), "I am taking Rhyan to tennis at 1234 Glenmoor Ct.");
}

#[tokio::test]
async fn offers_to_learn_unknown_words_not_yet_listed() {
    let engine = Engine::new("American English");
    let config = supereditr_core::Config::default();
    let job = Job {
        text: "Rhyan and Brynlee present the OKR update.".into(),
        preset: config.default_preset().clone(),
        steer: None,
        profile: None,
        style_notes: String::new(),
        dialect: "American English".into(),
        words: vec!["Brynlee".into()],
        code: Vec::new(),
        local_checks: true,
        builtin_ai: false,
    };
    let mut events = Vec::new();
    engine.run(job, &mut |e| events.push(e)).await;
    let Some(Event::Done(outcome)) = events.last() else { panic!("{events:?}") };
    assert_eq!(outcome.learnable, vec!["OKR", "Rhyan"]);
    assert!(
        outcome.text.starts_with("Ryan and Brynlee"),
        "a sentence-start name is still a possible typo: {}",
        outcome.text
    );
}

#[tokio::test]
async fn reports_when_nothing_can_check_the_text() {
    let engine = Engine::new("American English");
    let config = supereditr_core::Config::default();
    let job = Job {
        text: "This is an test.".into(),
        preset: config.default_preset().clone(),
        steer: None,
        profile: None,
        style_notes: String::new(),
        dialect: "American English".into(),
        words: Vec::new(),
        code: Vec::new(),
        local_checks: false,
        builtin_ai: false,
    };
    let mut events = Vec::new();
    engine.run(job, &mut |e| events.push(e)).await;
    assert!(matches!(events.last(), Some(Event::Failed { local_only: false, .. })), "{events:?}");
}

#[tokio::test]
async fn the_offline_checker_catches_what_the_model_missed() {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"<text>Their going to love the KPI dashboard.</text>\"}}]}\n\n\
                data: [DONE]\n\n";
    let (url, _) = mock_server(vec![(200, "text/event-stream", body.into())]).await;
    let engine = Engine::new("American English");
    let config = supereditr_core::Config::default();
    let job = Job {
        text: "Their going to love teh KPI dashboard.".into(),
        preset: config.default_preset().clone(),
        steer: None,
        profile: Some(profile(ProviderKind::OpenAiCompatible, &url, "local-model")),
        style_notes: String::new(),
        dialect: "American English".into(),
        words: Vec::new(),
        code: Vec::new(),
        local_checks: true,
        builtin_ai: false,
    };
    let mut events = Vec::new();
    engine.run(job, &mut |e| events.push(e)).await;
    let Some(Event::Done(outcome)) = events.last() else { panic!("{events:?}") };
    assert_eq!(outcome.text, "They're going to love the KPI dashboard.", "rule fix added; KPI left alone");
}

#[tokio::test]
async fn checks_whether_an_ai_can_answer_before_it_is_used() {
    let providers = Providers::new();
    let mut gemini = profile(ProviderKind::Gemini, "https://example.invalid", "gemini-x");
    gemini.api_key = None;
    assert_eq!(providers.check(&gemini).await.unwrap_err(), "Add an API key");

    let down = profile(ProviderKind::OpenAiCompatible, "http://127.0.0.1:9/v1", "m");
    assert!(providers.check(&down).await.unwrap_err().starts_with("Can’t connect"));

    let (url, _) = mock_server(vec![(200, "application/json", r#"{"data":[]}"#.into())]).await;
    let empty = profile(ProviderKind::OpenAiCompatible, &url, "local-model");
    assert_eq!(providers.check(&empty).await.unwrap_err(), "This server lists no models");

    let body = r#"{"data":[{"id":"local-model"}]}"#;
    let (url, _) = mock_server(vec![(200, "application/json", body.into())]).await;
    let up = profile(ProviderKind::OpenAiCompatible, &url, "local-model");
    assert_eq!(providers.check(&up).await.unwrap(), vec!["local-model"]);
}
