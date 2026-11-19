//! The names the UI calls and listens for, and the TypeScript definitions of what it receives.
//!
//! tests/contracts/ipc-names.json lists every command the app registers and every event it emits;
//! ui/src/lib/contracts.test.ts checks that ui/src/lib/ipc-names.ts names the same ones, and the UI's
//! `invoke` and `listen` wrappers take only those names. These tests check the Rust side: the
//! commands in `generate_handler!` (lib.rs) and the event names in the app's source.
//!
//! The payload types derive `ts_rs::TS` in tests, and each one marked `ts(export)` has a test that
//! writes its definition, and those of the types it uses, to ui/src/lib/generated (the directory is
//! set in .cargo/config.toml). `scripts/check.sh` fails when the committed files differ.

use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use ts_rs::TS;

/// The windows that receive events, which name each event `<window>:<event>`.
const WINDOWS: [&str; 4] = ["overlay", "settings", "update", "guide"];

fn fixture(list: &str) -> BTreeSet<String> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/contracts/ipc-names.json");
    let names: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    names[list].as_array().unwrap().iter().map(|name| name.as_str().unwrap().to_string()).collect()
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

/// Every string literal in the app's source that names an event: a window, a colon and a name in
/// lowercase words joined by hyphens, such as `"overlay:replace-anyway"`. Log lines such as
/// `"update: idle"` aren't names.
fn emitted_events() -> BTreeSet<String> {
    let mut files = Vec::new();
    rust_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let mut events = BTreeSet::new();
    for file in files.iter().filter(|f| !f.ends_with("ipc_contract.rs")) {
        let text = source(file);
        for window in WINDOWS {
            let opening = format!("\"{window}:");
            let mut rest = text.as_str();
            while let Some(at) = rest.find(&opening) {
                rest = &rest[at + opening.len()..];
                let length = rest.find(|c: char| !(c.is_ascii_lowercase() || c == '-')).unwrap_or(rest.len());
                if length > 0 && rest[length..].starts_with('"') {
                    events.insert(format!("{window}:{}", &rest[..length]));
                }
            }
        }
    }
    events
}

#[test]
fn the_fixture_lists_every_registered_command() {
    assert_eq!(
        registered_commands(),
        fixture("commands"),
        "update tests/contracts/ipc-names.json and ui/src/lib/ipc-names.ts"
    );
}

#[test]
fn the_fixture_lists_every_event_the_app_emits() {
    assert_eq!(
        emitted_events(),
        fixture("events"),
        "update tests/contracts/ipc-names.json and ui/src/lib/ipc-names.ts"
    );
}

/// Payloads from the core crate that no exported type here contains.
#[test]
fn export_bindings_core_payloads() {
    let config = ts_rs::Config::from_env();
    supereditr_core::provider::PullProgress::export_all(&config).unwrap();
}
