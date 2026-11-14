//! Apple Intelligence as the built-in AI: it reviews when nothing else is chosen and takes over when
//! the chosen AI can't answer. Uses a mock helper through `SUPEREDITR_FM_HELPER`, which is set only in a child process.
//! The mock is a shell script, so this runs on Unix only.
#![cfg(unix)]

use supereditr_core::engine::{Engine, Event, Job};
use supereditr_core::prompts::builtin_presets;
use supereditr_core::provider::{ProviderKind, ProviderProfile};

fn mock_helper() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("supereditr-builtin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("supereditr-fm");
    std::fs::write(
        &script,
        r#"#!/bin/sh
printf '%s\n' '{"type":"hello","available":true}'
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"type":"status"'*) printf '{"type":"status","id":%s,"available":true}\n' "$id" ;;
    *'"type":"request"'*) printf '{"type":"done","id":%s,"text":"They are here."}\n' "$id" ;;
  esac
done
"#,
    )
    .unwrap();
    std::process::Command::new("chmod").arg("+x").arg(&script).status().unwrap();
    script
}

async fn review(engine: &Engine, profile: Option<ProviderProfile>) -> Vec<Event> {
    let job = Job {
        text: "Their here.".into(),
        preset: builtin_presets().into_iter().find(|p| p.id == "polish").unwrap(),
        steer: None,
        profile,
        style_notes: String::new(),
        dialect: "American English".into(),
        words: Vec::new(),
        code: Vec::new(),
        local_checks: false,
        builtin_ai: true,
    };
    let mut events = Vec::new();
    engine.run(job, &mut |event| events.push(event)).await;
    events
}

fn done_source(events: &[Event]) -> String {
    match events.last() {
        Some(Event::Done(outcome)) => outcome.source.clone(),
        other => panic!("expected a finished review, got {other:?}"),
    }
}

#[tokio::test]
async fn apple_intelligence_is_the_default_and_the_backup() {
    // Start this test in its own process so the helper override cannot affect other providers.
    const CHILD: &str = "SUPEREDITR_BUILTIN_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let thread = std::thread::current();
        let test_name = thread.name().expect("libtest names the test thread");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test_name])
            .env(CHILD, "1")
            .env("SUPEREDITR_FM_HELPER", mock_helper())
            .status()
            .unwrap();
        assert!(status.success(), "built-in provider test failed: {status}");
        return;
    }
    let engine = Engine::new("American English");

    let events = review(&engine, None).await;
    assert_eq!(done_source(&events), "Apple Intelligence", "no AI chosen: the built-in one reviews");

    // A chosen AI that can't be reached (nothing listens on port 9) hands over before writing.
    let mut offline = ProviderProfile::new(ProviderKind::OpenAiCompatible);
    offline.base_url = "http://127.0.0.1:9/v1".into();
    let events = review(&engine, Some(offline)).await;
    assert_eq!(done_source(&events), "Apple Intelligence (backup)");
    let Some(Event::Done(outcome)) = events.last() else { unreachable!() };
    assert_eq!(outcome.text, "They are here.");
}
