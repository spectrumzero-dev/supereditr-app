//! Checks tests/contracts/ui-wording.json: the words in SuperEditr's windows and menus that the
//! end-to-end UI drivers look for.
//!
//! These tests check that each driver pattern matches the fixture's text the way the driver
//! matches it and is still in the driver's source, that the app's Rust code makes the text of each
//! `rust` entry, and that each `source` entry's literal is still in its file. The UI makes the
//! `ui` entries' text; ui/src/lib/ui-wording.svelte.test.ts renders it.

use regex::Regex;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;
use supereditr_core::Config;

use crate::platform::PlatformError;
use crate::updater::Status;

fn repository() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
}

fn read(file: &str) -> String {
    std::fs::read_to_string(repository().join(file)).unwrap_or_else(|e| panic!("{file}: {e}"))
}

fn entries() -> Vec<Value> {
    let fixture: Value = serde_json::from_str(&read("tests/contracts/ui-wording.json")).unwrap();
    fixture["wording"].as_array().unwrap().clone()
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_else(|| panic!("{key} missing in {value}"))
}

/// The entry's text where `driver` reads it: the one text, or the driver's own.
fn shown<'a>(entry: &'a Value, driver: &str) -> &'a str {
    match &entry["text"] {
        Value::String(text) => text,
        Value::Object(by_driver) => {
            by_driver[driver].as_str().unwrap_or_else(|| panic!("{}: no text for {driver}", text(entry, "id")))
        }
        other => panic!("{}: text is {other}", text(entry, "id")),
    }
}

/// A window's title in tauri.conf.json.
fn window_title(label: &str) -> String {
    let config: Value = serde_json::from_str(&read("src-tauri/tauri.conf.json")).unwrap();
    let windows = config["app"]["windows"].as_array().unwrap();
    let window = windows.iter().find(|w| w["label"] == label).unwrap_or_else(|| panic!("no {label} window"));
    text(window, "title").to_string()
}

/// The text of the `rust` entry `id`, as the app makes it.
fn made(id: &str) -> String {
    let version = || "99.0.0".to_string();
    let config = Config::default();
    match id {
        "unconfirmed" => PlatformError::Unverified.to_string(),
        "password-field" => PlatformError::SecureField.to_string(),
        // Built from pieces, so no complete token is in the source.
        "secret" => supereditr_core::sensitive::find(&format!(
            "Deploy with {}{} before Friday.",
            "ghp_", "Ab3dE5gH7jK9mN1pQ3sT5vW7yZ9bC1dF2hJ4"
        ))
        .expect("the token is found")
        .message(),
        "not-responding" => crate::platform::log_lines::not_responding("RacePad").to_string(),
        "panel-title" => window_title("overlay"),
        "settings-title" => window_title("settings"),
        "update-title" => crate::updater::TITLE.to_string(),
        "menu-turn-off" => crate::app_toggle::label_for(&config, "com.apple.TextEdit", "TextEdit").unwrap(),
        "menu-turn-on" => {
            let mut config = config;
            config.excluded_apps.push("com.apple.TextEdit".into());
            crate::app_toggle::label_for(&config, "com.apple.TextEdit", "TextEdit").unwrap()
        }
        "menu-check-for-updates" => Status::Idle.menu_line(),
        "menu-update-failed" => Status::InstallFailed { version: version() }.menu_line(),
        "menu-restart-to-update" => Status::Ready { version: version() }.menu_line(),
        "menu-downloading" => Status::Downloading { version: version(), percent: Some(40) }.menu_line(),
        // Recording Quick fix's shortcut for Review selection, as the macOS driver does.
        "shortcut-taken" => {
            crate::commands::shortcut_problem(&config, &config.hotkeys.quick_fix, "review", Vec::new, || None, || false)
                .expect("Quick fix's shortcut is refused")
        }
        other => panic!("tests/contracts/ui-wording.json says Rust makes {other}, which these tests don't make"),
    }
}

#[test]
fn each_entry_is_listed_once_with_a_known_maker() {
    let mut ids = BTreeSet::new();
    for entry in entries() {
        let id = text(&entry, "id");
        assert!(ids.insert(id.to_string()), "{id} is listed twice");
        let file = text(&entry, "file");
        assert!(repository().join(file).is_file(), "{id}: no {file}");
        assert!(matches!(text(&entry, "made_by"), "ui" | "rust" | "source"), "{id}: made_by is {}", entry["made_by"]);
        assert!(
            !entry["matched_by"].as_array().unwrap().is_empty(),
            "{id}: no driver matches it, so it needn't be listed"
        );
    }
}

#[test]
fn each_rust_entry_is_made_as_the_fixture_says() {
    for entry in entries().iter().filter(|e| e["made_by"] == "rust") {
        let id = text(entry, "id");
        assert_eq!(made(id), text(entry, "text"), "{id}: update tests/contracts/ui-wording.json and the drivers");
    }
}

/// `literal` with `vars` put in.
fn filled(literal: &str, vars: &Value) -> String {
    let mut filled = literal.to_string();
    if let Some(vars) = vars.as_object() {
        for (name, value) in vars {
            assert!(filled.contains(name.as_str()), "{name} isn't in {literal}");
            filled = filled.replace(name.as_str(), value.as_str().unwrap());
        }
    }
    filled
}

#[test]
fn each_source_entry_is_in_its_file() {
    for entry in entries().iter().filter(|e| e["made_by"] == "source") {
        let (id, file, literal) = (text(entry, "id"), text(entry, "file"), text(entry, "literal"));
        assert!(
            has_whole_literal(&read(file), literal),
            "{id}: {file} no longer has {literal:?} as a whole literal; update tests/contracts/ui-wording.json and the drivers"
        );
        assert_eq!(
            filled(literal, &entry["vars"]),
            text(entry, "text"),
            "{id}: the literal with its vars isn't the text"
        );
    }
}

/// A PowerShell `-like` pattern as a regular expression: the whole text, ignoring case.
fn wildcard(pattern: &str) -> Regex {
    let body: String = pattern
        .chars()
        .map(|c| match c {
            '*' => ".*".to_string(),
            '?' => ".".to_string(),
            c => regex::escape(&c.to_string()),
        })
        .collect();
    Regex::new(&format!("(?is)^{body}$")).unwrap()
}

fn matches(kind: &str, pattern: &str, text: &str) -> bool {
    match kind {
        "substring" => text.contains(pattern),
        "regex" => Regex::new(pattern).unwrap().is_match(text),
        "regex-ignore-case" => Regex::new(&format!("(?i){pattern}")).unwrap().is_match(text),
        "wildcard" => wildcard(pattern).is_match(text),
        "equals" => text == pattern,
        "prefix" => text.starts_with(pattern),
        "probe-button" => match pattern.strip_prefix('~') {
            Some(part) => text.contains(part),
            None => text.trim() == pattern,
        },
        other => panic!("unknown kind {other}"),
    }
}

#[test]
fn the_kinds_match_as_the_drivers_do() {
    assert!(matches("wildcard", "Find my AI accounts*", "find my ai accounts →"));
    assert!(!matches("wildcard", "Use", "Use this"));
    assert!(matches("wildcard", "*replace*", "Alt+Enter replace"));
    assert!(matches("probe-button", "~replace", "Alt+Enter replace"));
    assert!(!matches("probe-button", "Review", "Review selection"));
    assert!(matches("regex-ignore-case", "password", "SuperEditr doesn’t read Password fields."));
    assert!(!matches("regex", "\\breplace\\b", "⌥↩ replaces your text"));
}

#[test]
fn each_driver_pattern_matches_its_text() {
    for entry in entries() {
        let id = text(&entry, "id");
        for matcher in entry["matched_by"].as_array().unwrap() {
            let driver = text(matcher, "driver");
            let shown = shown(&entry, driver);
            let pattern = filled(text(matcher, "pattern"), &matcher["vars"]);
            assert!(
                matches(text(matcher, "kind"), &pattern, shown),
                "{id}: {driver}'s {pattern:?} doesn't match {shown:?}"
            );
        }
    }
}

/// Whether `source` has `pattern` as a whole literal: with a quote or a regex slash right before and
/// right after it. A pattern found only inside a longer literal isn't what the driver matches.
fn has_whole_literal(source: &str, pattern: &str) -> bool {
    let delimiter = |c: Option<char>| matches!(c, Some('\'' | '"' | '/'));
    source.match_indices(pattern).any(|(at, found)| {
        delimiter(source[..at].chars().next_back()) && delimiter(source[at + found.len()..].chars().next())
    })
}

#[test]
fn a_pattern_inside_a_longer_literal_is_not_the_drivers() {
    assert!(has_whole_literal("Settings-Press 'Use' | Out-Null", "Use"));
    assert!(!has_whole_literal("Settings-Press 'Use this' | Out-Null", "Use"));
    assert!(!has_whole_literal("text.contains(\"Scan again\")", "Scan"));
}

#[test]
fn each_driver_pattern_is_in_the_driver() {
    for entry in entries() {
        for matcher in entry["matched_by"].as_array().unwrap() {
            let file = text(matcher, "file");
            assert!(file.starts_with("tests/ui/") || file.starts_with("scripts/ui-smoke"), "{file} isn't a driver");
            let pattern = text(matcher, "pattern");
            assert!(
                has_whole_literal(&read(file), pattern),
                "{}: {file} no longer has {pattern:?} as a whole literal; update tests/contracts/ui-wording.json to what it matches now",
                text(&entry, "id")
            );
        }
    }
}
