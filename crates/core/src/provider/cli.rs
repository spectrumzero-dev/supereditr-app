//! Claude Code CLI bridge. It runs the user's own signed-in `claude` binary (a separate install,
//! or the copy that comes with the Claude desktop app) in headless print mode, so a Claude
//! subscription works without an API key. Process start-up makes it slower
//! than a direct API call, and the settings UI says so.

use super::{ChatRequest, ProviderError, ProviderProfile};
use crate::process::{self, Stdin};
use crate::sse::LineParser;
use serde_json::Value;
use std::io;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;

const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const DIAGNOSTIC_BYTES: usize = 8192;

mod prompt_file;

pub(super) async fn stream_claude(
    profile: &ProviderProfile,
    request: &ChatRequest,
    on_delta: &mut (dyn FnMut(&str) + Send),
) -> Result<String, ProviderError> {
    let program = crate::detect::claude_program(&profile.base_url);
    let program = program.as_str();
    let temp_dir = std::env::temp_dir();
    let system_prompt = prompt_file::write(&request.system, &temp_dir)
        .map_err(|e| ProviderError::Process(format!("Couldn’t prepare a private Claude CLI system prompt: {e}")))?;
    let prompt_path = system_prompt
        .path()
        .to_str()
        .ok_or_else(|| ProviderError::Process("The Claude CLI system prompt path must be valid Unicode.".into()))?;
    let args = [
        "-p",
        "--model",
        &profile.model,
        "--system-prompt-file",
        prompt_path,
        "--tools",
        "",
        "--strict-mcp-config",
        "--no-session-persistence",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
    ];
    // Without SuperEditr's Accessibility permission: see `crate::process`.
    let child = process::spawn(program, &args, Some(&temp_dir), Stdin::Piped)
        .map_err(|e| ProviderError::Process(format!("Couldn’t start the Claude CLI ({program}): {e}")))?;
    // The first model delta confirms the CLI has read the system prompt. Until then the
    // guard also removes the file if the exchange fails or the caller cancels it.
    let mut prompt = Some(system_prompt);
    exchange(
        child,
        request.user.as_bytes(),
        &mut |text| {
            drop(prompt.take());
            on_delta(text);
        },
        IDLE_TIMEOUT,
    )
    .await
}

/// Drive all three pipes together. A child can fill stderr or stdout before it reads stdin.
async fn exchange(
    mut child: process::Child,
    input: &[u8],
    on_delta: &mut (dyn FnMut(&str) + Send),
    idle: Duration,
) -> Result<String, ProviderError> {
    let mut stdin = child.stdin.take();
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut input = input;
    let mut stdin_error = None;
    if input.is_empty() {
        stdin = None;
    }
    let mut parser = LineParser::default();
    let mut reply = Reply::default();
    let mut diagnostic = Vec::new();
    let mut output = [0; 8192];
    let mut errors = [0; DIAGNOSTIC_BYTES];
    let mut status = None;
    let deadline = tokio::time::sleep(idle);
    tokio::pin!(deadline);
    let result = async {
        while status.is_none() || stdout.is_some() || stderr.is_some() || stdin.is_some() {
            tokio::select! {
                _ = &mut deadline => {
                    return Err(ProviderError::Process("The Claude CLI stopped responding.".into()));
                }
                written = async { stdin.as_mut().expect("stdin is open").write(input).await }, if stdin.is_some() => {
                    match written {
                        Ok(n) if n > 0 => input = &input[n..],
                        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                            stdin_error = Some(error);
                            input = &[];
                        }
                        Err(error) => return Err(process_error(error)),
                        Ok(_) => return Err(process_error(io::ErrorKind::WriteZero.into())),
                    }
                    if input.is_empty() {
                        stdin = None;
                    }
                }
                read = async { stdout.as_mut().expect("stdout is open").read(&mut output).await }, if stdout.is_some() => {
                    match read.map_err(process_error)? {
                        0 => {
                            stdout = None;
                            if let Some(line) = parser.finish() {
                                reply.line(&line, on_delta)?;
                            }
                        }
                        n => {
                            for line in parser.push(&output[..n]) {
                                reply.line(&line?, on_delta)?;
                            }
                        }
                    }
                }
                read = async { stderr.as_mut().expect("stderr is open").read(&mut errors).await }, if stderr.is_some() => {
                    match read.map_err(process_error)? {
                        0 => stderr = None,
                        n => {
                            let overflow = (diagnostic.len() + n).saturating_sub(DIAGNOSTIC_BYTES);
                            diagnostic.drain(..overflow);
                            diagnostic.extend_from_slice(&errors[..n]);
                        }
                    }
                }
                exited = child.wait(), if status.is_none() => {
                    status = Some(exited.map_err(process_error)?);
                }
            }
            deadline.as_mut().reset(Instant::now() + idle);
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        drop((stdin, stdout, stderr));
        let _ = child.terminate().await;
        return Err(error);
    }
    if !status.expect("the child exited").success() {
        let diagnostic = String::from_utf8_lossy(&diagnostic);
        if diagnostic.lines().any(|line| line.contains("--system-prompt-file") && line.contains("unknown option")) {
            return Err(ProviderError::Process(
                "This Claude CLI doesn’t support --system-prompt-file. Run `claude update` or select a newer Claude CLI in Settings.".into(),
            ));
        }
        let detail = diagnostic.lines().last().unwrap_or("no output");
        return Err(ProviderError::Process(format!("The Claude CLI exited with an error: {detail}")));
    }
    if let Some(e) = stdin_error {
        return Err(ProviderError::Process(e.to_string()));
    }
    // The result event carries the complete revision; streamed fragments are only a preview.
    match reply.result {
        Some(text) => {
            if !reply.streamed {
                on_delta(&text);
            }
            Ok(text)
        }
        None => Err(ProviderError::Incomplete),
    }
}

fn process_error(error: io::Error) -> ProviderError {
    ProviderError::Process(error.to_string())
}

#[derive(Default)]
struct Reply {
    streamed: bool,
    result: Option<String>,
}

impl Reply {
    fn line(&mut self, line: &str, on_delta: &mut (dyn FnMut(&str) + Send)) -> Result<(), ProviderError> {
        let Ok(value) = serde_json::from_str::<Value>(line) else { return Ok(()) };
        match value["type"].as_str() {
            Some("stream_event") => {
                let event = &value["event"];
                if event["type"] == "content_block_delta"
                    && event["delta"]["type"] == "text_delta"
                    && let Some(text) = event["delta"]["text"].as_str()
                {
                    self.streamed |= !text.is_empty();
                    on_delta(text);
                }
            }
            Some("result") => {
                if value["is_error"].as_bool() == Some(true) {
                    let message = value["result"].as_str().unwrap_or("the Claude CLI reported an error");
                    return Err(ProviderError::Process(message.to_string()));
                }
                self.result = value["result"].as_str().map(str::to_string);
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn shell(script: &str) -> process::Child {
        process::spawn("/bin/sh", &["-c", script], Some(&std::env::temp_dir()), Stdin::Piped).unwrap()
    }

    #[tokio::test]
    async fn drains_a_chatty_error_before_the_child_reads_stdin() {
        let child = shell("head -c 262144 /dev/zero | tr '\\000' x >&2; cat > /dev/null; exit 2");
        let input = vec![b'a'; 128 * 1024];
        let error = exchange(child, &input, &mut |_| {}, Duration::from_secs(5)).await.unwrap_err();
        let detail = error.to_string();
        assert!(detail.contains("exited with an error"), "{detail}");
        assert!(detail.ends_with(&"x".repeat(DIAGNOSTIC_BYTES)), "the full tail was kept");
        assert!(detail.len() < DIAGNOSTIC_BYTES + 100, "diagnostics are bounded");
    }

    #[tokio::test]
    async fn times_out_when_the_child_stalls_during_the_exchange() {
        let child = shell("exec /bin/sleep 30");
        #[cfg(target_os = "macos")]
        let pid = child.id();
        let input = vec![b'a'; 128 * 1024];
        let error = exchange(child, &input, &mut |_| {}, Duration::from_millis(60)).await.unwrap_err();
        assert!(error.to_string().contains("stopped responding"), "{error}");
        #[cfg(target_os = "macos")]
        assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, -1, "the child was reaped");
    }

    #[tokio::test]
    async fn the_final_result_wins_over_the_streamed_preview() {
        let child = shell(concat!(
            "printf '%s\\n' '{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",",
            "\"delta\":{\"type\":\"text_delta\",\"text\":\"Preview\"}}}'; ",
            "printf '%s' '{\"type\":\"result\",\"result\":\"Final revision.\"}'"
        ));
        let mut preview = String::new();
        let result = exchange(child, b"", &mut |text| preview.push_str(text), Duration::from_secs(5)).await.unwrap();
        assert_eq!(preview, "Preview");
        assert_eq!(result, "Final revision.");
    }
}
