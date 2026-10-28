//! Claude Code CLI bridge. It runs the user's own signed-in `claude` binary in headless print
//! mode, so a Claude subscription works without an API key. Process start-up makes it slower
//! than a direct API call, and the settings UI says so.

use super::{ChatRequest, ProviderError, ProviderProfile};
use crate::sse::LineParser;
use serde_json::Value;
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

pub(super) async fn stream_claude(
    profile: &ProviderProfile,
    request: &ChatRequest,
    on_delta: &mut (dyn FnMut(&str) + Send),
) -> Result<String, ProviderError> {
    let program = if profile.base_url.is_empty() { "claude" } else { profile.base_url.as_str() };
    let mut child = Command::new(program)
        .args(["-p", "--model", &profile.model, "--system-prompt", &request.system])
        .args(["--tools", "", "--strict-mcp-config", "--no-session-persistence"])
        .args(["--output-format", "stream-json", "--verbose", "--include-partial-messages"])
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ProviderError::Process(format!("Couldn’t start the Claude CLI ({program}): {e}")))?;

    let mut stdin = child.stdin.take().expect("stdin is piped");
    stdin.write_all(request.user.as_bytes()).await.map_err(|e| ProviderError::Process(e.to_string()))?;
    drop(stdin);

    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut parser = LineParser::default();
    let mut streamed = String::new();
    let mut result: Option<String> = None;
    let mut buffer = [0u8; 8192];
    loop {
        let read = stdout.read(&mut buffer).await.map_err(|e| ProviderError::Process(e.to_string()))?;
        let lines = if read == 0 { parser.finish().into_iter().collect() } else { parser.push(&buffer[..read]) };
        for line in lines {
            let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
            match value["type"].as_str() {
                Some("stream_event") => {
                    let event = &value["event"];
                    if event["type"] == "content_block_delta"
                        && event["delta"]["type"] == "text_delta"
                        && let Some(text) = event["delta"]["text"].as_str()
                    {
                        streamed.push_str(text);
                        on_delta(text);
                    }
                }
                Some("result") => {
                    if value["is_error"].as_bool() == Some(true) {
                        let message = value["result"].as_str().unwrap_or("the Claude CLI reported an error");
                        return Err(ProviderError::Process(message.to_string()));
                    }
                    result = value["result"].as_str().map(str::to_string);
                }
                _ => {}
            }
        }
        if read == 0 {
            break;
        }
    }
    let status = child.wait().await.map_err(|e| ProviderError::Process(e.to_string()))?;
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr).await;
        }
        let detail = stderr.lines().last().unwrap_or("no output").to_string();
        return Err(ProviderError::Process(format!("The Claude CLI exited with an error: {detail}")));
    }
    // The result event carries the complete revision; streamed fragments are only a preview.
    match result {
        Some(text) => {
            if streamed.is_empty() {
                on_delta(&text);
            }
            Ok(text)
        }
        None => Err(ProviderError::Incomplete),
    }
}
