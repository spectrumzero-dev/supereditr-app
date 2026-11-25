//! The names the UI calls and listens for, and the TypeScript definitions of what it receives.
//!
//! tests/contracts/ipc-names.json lists the commands each window calls and the app's events.
//! ui/src/lib/contracts.test.ts checks the frontend call sites and names. These tests check the
//! registered commands, the grants Tauri enforces for each window, and the emitted events.
//!
//! The payload types derive `ts_rs::TS` in tests, and each one marked `ts(export)` has a test that
//! writes its definition, and those of the types it uses, to ui/src/lib/generated (the directory is
//! set in .cargo/config.toml). `scripts/check.sh` fails when the committed files differ.

use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tauri::ipc::{CallbackFn, InvokeResponseBody};
use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, noop_assets};
use tauri::webview::InvokeRequest;
use tauri::{Context, Url, WebviewWindow, WebviewWindowBuilder};
use ts_rs::TS;

/// String literals shaped like an event name that aren't one: credential account names in tests.
const NOT_EVENTS: [&str; 2] = ["anthropic:anthropic", "gemini:gemini"];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IpcNames {
    window_commands: BTreeMap<String, BTreeSet<String>>,
    events: BTreeSet<String>,
}

fn fixture() -> IpcNames {
    serde_json::from_str(include_str!("../../tests/contracts/ipc-names.json")).unwrap()
}

fn source(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

/// The commands `generate_handler!` registers, by name.
fn registered_commands() -> BTreeSet<String> {
    let lib = source(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"));
    let start = lib.find("generate_handler![").expect("lib.rs registers its commands") + "generate_handler![".len();
    let list = &lib[start..start + lib[start..].find(']').unwrap()];
    list.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.rsplit("::").next().unwrap().to_string())
        .collect()
}

/// Every string literal in the app's source shaped like an event name: lowercase letters, a colon,
/// then lowercase words joined by hyphens, such as `"overlay:replace-anyway"`. The window isn't
/// matched against a list, so a misspelt one (`"overaly:engine"`) is found too, and fails the
/// comparison with the fixture. Log lines such as `"update: idle"` aren't shaped like this.
fn emitted_events() -> BTreeSet<String> {
    let mut files = Vec::new();
    rust_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let mut events = BTreeSet::new();
    for file in files.iter().filter(|f| !f.ends_with("ipc_contract.rs")) {
        let text = source(file);
        // Every other piece between quotes is the inside of a literal, or near enough: a piece
        // that isn't one only ever adds a name the fixture lacks, which fails loudly.
        for literal in text.split('"') {
            if is_event_name(literal) && !NOT_EVENTS.contains(&literal) {
                events.insert(literal.to_string());
            }
        }
    }
    events
}

fn is_event_name(literal: &str) -> bool {
    let Some((window, name)) = literal.split_once(':') else { return false };
    !window.is_empty()
        && window.chars().all(|c| c.is_ascii_lowercase())
        && !name.is_empty()
        && name.chars().all(|c| c.is_ascii_lowercase() || c == '-')
}

#[test]
fn a_misspelt_window_is_still_an_event_name() {
    assert!(is_event_name("overaly:engine"));
    assert!(is_event_name("overlay:replace-anyway"));
    assert!(!is_event_name("update: idle"));
    assert!(!is_event_name("{kind}:{profile_id}"));
}

#[test]
fn the_fixture_lists_every_registered_command() {
    assert_eq!(
        registered_commands(),
        fixture().window_commands.into_values().flatten().collect(),
        "update tests/contracts/ipc-names.json and ui/src/lib/ipc-names.ts"
    );
}

#[test]
fn the_fixture_lists_every_event_the_app_emits() {
    assert_eq!(emitted_events(), fixture().events, "update tests/contracts/ipc-names.json and ui/src/lib/ipc-names.ts");
}

fn acl_context() -> Context<MockRuntime> {
    tauri::generate_context!("tauri.conf.json", assets = noop_assets(), test = true)
}

fn invoke(
    window: &WebviewWindow<MockRuntime>,
    url: &Url,
    command: &str,
    body: Value,
) -> Result<InvokeResponseBody, Value> {
    get_ipc_response(
        window,
        InvokeRequest {
            cmd: command.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: url.clone(),
            body: body.into(),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.into(),
        },
    )
}

fn assert_denied(result: Result<InvokeResponseBody, Value>, window: &str, command: &str) {
    let error = result.unwrap_err();
    assert!(error.as_str().is_some_and(|message| message.contains("not allowed")), "{window}: {command}: {error}");
}

#[test]
fn window_commands_are_authorized_by_tauri() {
    let names = fixture();
    let mut commands = registered_commands();
    commands.insert("unlisted_command".into());

    // Every app command has a harmless handler. A denial must come from Tauri before dispatch.
    let app = mock_builder()
        .invoke_handler(|invoke| {
            invoke.resolver.resolve(());
            true
        })
        .build(acl_context())
        .unwrap();
    let remote = "https://example.invalid/".parse().unwrap();

    for label in names.window_commands.keys().map(String::as_str).chain(["unlisted-window"]) {
        let window = WebviewWindowBuilder::new(&app, label, Default::default()).build().unwrap();
        let local = window.url().unwrap();
        for command in &commands {
            let result = invoke(&window, &local, command, json!({}));
            if names.window_commands.get(label).is_some_and(|allowed| allowed.contains(command)) {
                assert!(result.is_ok(), "{label}: {command}: {result:?}");
                assert_denied(invoke(&window, &remote, command, json!({})), label, command);
            } else {
                assert_denied(result, label, command);
            }
        }
    }
}

#[test]
fn windows_keep_their_required_plugin_permissions() {
    let app = mock_builder().build(acl_context()).unwrap();
    for label in fixture().window_commands.keys() {
        let window = WebviewWindowBuilder::new(&app, label, Default::default()).build().unwrap();
        let local = window.url().unwrap();
        let listen = invoke(
            &window,
            &local,
            "plugin:event|listen",
            json!({"event": "acl-test", "target": {"kind": "Any"}, "handler": 2}),
        )
        .unwrap();
        let event_id = listen.deserialize::<u32>().unwrap();
        invoke(&window, &local, "plugin:event|unlisten", json!({"event": "acl-test", "eventId": event_id})).unwrap();
        let emit = "plugin:event|emit";
        assert_denied(invoke(&window, &local, emit, json!({"event": "acl-test"})), label, emit);

        for command in ["plugin:window|start_dragging", "plugin:window|internal_toggle_maximize"] {
            let result = invoke(&window, &local, command, json!({}));
            if label == "settings" {
                assert!(result.is_ok(), "{label}: {command}: {result:?}");
            } else {
                assert_denied(result, label, command);
            }
        }

        // Inspect the URL scope so the test never opens a browser.
        let opener = window.resolve_command_scope::<Value>("opener", "open_url").unwrap();
        if label == "settings" {
            let scope = opener.unwrap();
            let allowed: Vec<_> = scope.command_scope().allows().iter().map(|value| value.as_ref()).collect();
            assert_eq!(allowed, [&json!({"url": "https://*"})]);
        } else {
            assert!(opener.is_none(), "{label} can open URLs");
        }
    }
}

/// Payloads from the core crate that no exported type here contains.
#[test]
fn export_bindings_core_payloads() {
    let config = ts_rs::Config::from_env();
    supereditr_core::provider::PullProgress::export_all(&config).unwrap();
}
