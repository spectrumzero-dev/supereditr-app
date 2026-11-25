use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use supereditr_core::provider::{ChatRequest, ProviderKind, ProviderProfile, Providers};
use tempfile::TempDir;

fn request() -> ChatRequest {
    ChatRequest {
        system: "  Style notes: use Brynlee’s phrasing.\r\nDictionary: QuillPoint\n\n".into(),
        user: "\n<text>Rhyan’s draft has a typo.</text>\r\n  ".into(),
        max_tokens: 100,
        guided: false,
    }
}

fn fake_claude(body: &str) -> (TempDir, ProviderProfile) {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("fake claude");
    let setup = r#"#!/bin/sh
set -eu
cd "$(dirname "$0")"
printf '.\n' >> launches
[ "$4" = --system-prompt-file ]
printf '%s\000' "$@" > arguments
printf '%s\n' "$5" > prompt-path
"#;
    fs::write(&script, format!("{setup}{body}\n")).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let mut profile = ProviderProfile::new(ProviderKind::ClaudeCli);
    profile.base_url = script.to_str().unwrap().into();
    (dir, profile)
}

fn prompt_path(dir: &Path) -> PathBuf {
    let path = fs::read_to_string(dir.join("prompt-path")).unwrap();
    PathBuf::from(path.strip_suffix('\n').unwrap())
}

#[tokio::test]
async fn claude_prompt_uses_exact_separate_channels_and_cleans_up_during_streaming() {
    let (dir, profile) = fake_claude(
        r#"cat > seen-user
cat "$5" > seen-system
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Preview"}}}'
while [ ! -f preview-seen ]; do /bin/sleep 0.01; done
printf '%s\n' '{"type":"result","is_error":false,"result":"Final text"}'"#,
    );
    let request = request();
    let mut deltas = Vec::new();
    let mut on_delta = |delta: &str| {
        assert!(!prompt_path(dir.path()).exists());
        deltas.push(delta.to_owned());
        fs::write(dir.path().join("preview-seen"), []).unwrap();
    };
    let providers = Providers::new();
    let review = providers.stream(&profile, &request, &mut on_delta);
    let text = tokio::time::timeout(Duration::from_secs(5), review).await.expect("the preview did not stream").unwrap();
    assert_eq!(text, "Final text");
    assert_eq!(deltas, ["Preview"]);
    assert_eq!(fs::read(dir.path().join("seen-system")).unwrap(), request.system.as_bytes());
    assert_eq!(fs::read(dir.path().join("seen-user")).unwrap(), request.user.as_bytes());

    let path = prompt_path(dir.path());
    assert!(path.is_absolute());
    let args = fs::read_to_string(dir.path().join("arguments")).unwrap();
    assert!(args.split_terminator('\0').eq([
        "-p",
        "--model",
        &profile.model,
        "--system-prompt-file",
        path.to_str().unwrap(),
        "--tools",
        "",
        "--strict-mcp-config",
        "--no-session-persistence",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
    ]));
}

#[tokio::test]
async fn claude_prompt_is_removed_on_a_cli_error_before_any_delta() {
    let (dir, profile) = fake_claude(
        r#"cat > /dev/null
printf '%s\n' '{"type":"result","is_error":true,"result":"Synthetic failure"}'
exec /bin/sleep 30"#,
    );
    let error = Providers::new().stream(&profile, &request(), &mut |_| {}).await.unwrap_err();
    assert!(error.to_string().contains("Synthetic failure"), "{error}");
    assert!(!prompt_path(dir.path()).exists());
}

#[tokio::test]
async fn claude_prompt_is_removed_when_an_in_flight_review_is_dropped() {
    let (dir, profile) = fake_claude("cat > /dev/null\nexec /bin/sleep 30");
    let providers = Providers::new();
    let request = request();
    let mut on_delta = |_: &str| {};
    let mut review = Box::pin(providers.stream(&profile, &request, &mut on_delta));
    let path = tokio::select! {
        result = &mut review => panic!("review ended before cancellation: {result:?}"),
        path = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(path) = fs::read_to_string(dir.path().join("prompt-path"))
                    && let Some(path) = path.strip_suffix('\n')
                {
                    break PathBuf::from(path);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }) => path.expect("the fake Claude CLI did not start"),
    };
    assert_eq!(fs::read(&path).unwrap(), request.system.as_bytes());
    drop(review);
    assert!(!path.exists());
}

#[tokio::test]
async fn claude_prompt_file_unsupported_recommends_updating_without_retrying() {
    let (dir, profile) = fake_claude(
        r#"printf "%s\n" "error: unknown option '--system-prompt-file'" >&2
exit 1"#,
    );
    let mut request = request();
    // The old CLI exits without reading stdin. A large input hits the broken pipe reliably.
    request.user = "QuillPoint\n".repeat(100_000);
    let error = Providers::new().stream(&profile, &request, &mut |_| {}).await.unwrap_err();
    assert!(error.to_string().contains("claude update"), "{error}");
    assert_eq!(fs::read_to_string(dir.path().join("launches")).unwrap().lines().count(), 1);
    assert!(!prompt_path(dir.path()).exists());
}
