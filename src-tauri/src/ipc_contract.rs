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

/// String literals shaped like an event name that aren't one: credential account names in tests.
const NOT_EVENTS: [&str; 2] = ["anthropic:anthropic", "gemini:gemini"];

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
