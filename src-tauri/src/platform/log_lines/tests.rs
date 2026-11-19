//! Checks tests/contracts/log-lines.json against the types that write each line and the drivers
//! that read it (see the module's documentation).

use super::*;
use regex::Regex;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

fn repository() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
}

fn fixture() -> Value {
    let path = repository().join("tests/contracts/log-lines.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn lines() -> Vec<Value> {
    fixture()["lines"].as_array().unwrap().clone()
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_else(|| panic!("{key} missing in {value}"))
}

/// The line `id` in the fixture, as the types here write it.
fn written(id: &str) -> String {
    let ms = Duration::from_millis;
    // Stand-ins for types that only exist on one platform, with the same `Debug`.
    #[derive(Debug)]
    enum Action {
        Review,
    }
    let read = |app, chars, of_field, via, element, took, access| {
        Read { how: how(true), app, chars, of_field, via, element, took, access }.to_string()
    };
    let replaced = |app, outcome: &ReplaceOutcome, took| Replaced { app, anyway: false, outcome, took }.to_string();
    match id {
        "read-accessibility" => {
            read("com.google.Chrome", 42, Some(42), "Accessibility", "", ms(12), "replacing allowed")
        }
        "read-at-spi" => read("org.gnome.TextEditor", 1234, None, "AT-SPI", "", ms(40), "pasting allowed"),
        "read-guarded-copy-linux" => read("foot", 15, None, "a guarded copy", "", ms(220), "copy-only"),
        "read-ui-automation" => read(
            "notepad.exe",
            27,
            None,
            "UI Automation",
            " (UIA RichEditD2DPT element of process 4321, type 50030)",
            ms(30),
            "KnownSafe",
        ),
        "read-guarded-copy-windows" => {
            read("WindowsTerminal.exe", 15, None, "a guarded copy", "", ms(240), "copy-only")
        }
        "read-failed-not-responding" => ReadFailed {
            how: how(true),
            app: "org.gnome.TextEditor",
            error: &not_responding("Text Editor"),
            took: ms(2500),
        }
        .to_string(),
        "read-failed-password" => {
            ReadFailed { how: how(true), app: "quillpoint.exe", error: &PlatformError::SecureField, took: ms(20) }
                .to_string()
        }
        "replace-applied-linux" => replaced("org.gnome.TextEditor", &ReplaceOutcome::Applied, ms(180)),
        "replace-applied-windows" => replaced("notepad.exe", &ReplaceOutcome::Applied, ms(250)),
        "replace-uncertain" => replaced("racepad.exe", &ReplaceOutcome::Uncertain(PlatformError::Unverified), ms(1200)),
        "replace-clipboard-kept" => {
            let why = PlatformError::ClipboardUnrestorable("The clipboard couldn’t be put aside.".into());
            replaced("notepad.exe", &ReplaceOutcome::ClipboardKept(why), ms(90))
        }
        "replace-stale" => replaced("notepad.exe", &ReplaceOutcome::CopyOnly(PlatformError::Stale), ms(40)),
        "replace-in-a-terminal" => replaced("WindowsTerminal.exe", &ReplaceOutcome::Applied, ms(300)),
        "focus-not-back" => FocusReturned { back: false, took: ms(1500) }.to_string(),
        "applied-in-place" => AppliedInPlace {
            app: "com.google.Chrome",
            edits: 2,
            ms: 35,
            native: false,
            verified: true,
            code: "; 2 of 2 code spans still code",
        }
        .to_string(),
        "applied-partly" => {
            AppliedPartly { app: "com.apple.TextEdit", applied: 1, total: 2, reason: "focus left the field" }
                .to_string()
        }
        "applying-whole" => {
            ApplyingWhole { app: "com.google.Chrome", reason: &"the field has no formatting to keep" }.to_string()
        }
        "glide" => Glide {
            how: "done",
            driver: "display link",
            distance: 130.0,
            steps: &[(16.7, 17.1, 40.25), (33.3, 33.9, 89.75)],
        }
        .to_string(),
        "code-in-the-field" => {
            CodeInField { pieces: 2, took: ms(3), stopped_early: false, subroles: "AXCodeStyleGroup" }.to_string()
        }
        "over-the-maximum" => Automatic {
            app: Some("com.apple.TextEdit"),
            gesture: &super::super::SelectionGesture::SelectAll,
            // The driver looks for the default maximum.
            outcome: &OverMaximum { chars: 3000, max: supereditr_core::config::Config::default().auto_max_chars },
        }
        .to_string(),
        "pasted-in-place" => PastedInPlace { app: "chromium", changes: 2, writes: 2, ms: 310 }.to_string(),
        "pasted-in-place-in-steps" => PastedInPlace { app: "chromium", changes: 5, writes: 4, ms: 640 }.to_string(),
        "pasted-whole-too-many-writes" => {
            PastedWhole { app: "chromium", why: &TooManyWrites { changes: 6, writes: 5, max: 4 } }.to_string()
        }
        "pasted-whole-too-many-changes" => PastedWhole {
            app: "chromium",
            why: &supereditr_core::edits::Whole::TooMany(supereditr_core::edits::MAX_EDITS + 1),
        }
        .to_string(),
        "pasted-formatting" => PastedFormatting { app: "chromium", html: true }.to_string(),
        "panel-opens" => {
            PanelPlaced { shown: false, at: Rect { x: 412.0, y: 230.0, width: 548.0, height: 186.0 } }.to_string()
        }
        "panel-moves" => {
            PanelPlaced { shown: true, at: Rect { x: 420.4, y: 236.6, width: 548.0, height: 190.0 } }.to_string()
        }
        "panel-focused" => PanelFocused { took: ms(85) }.to_string(),
        "shortcut-from-the-portal" => ShortcutFromPortal { action: "review" }.to_string(),
        "main-thread-silent" => {
            MainThreadSilent { silent: ms(5200), command: "in no SuperEditr command", wait: "thread unknown" }
                .to_string()
        }
        "hyprland-reloaded" => HyprlandReloaded { binds: 2 }.to_string(),
        "webview-stopped" => WebviewStopped { window: "overlay", process: RENDERER }.to_string(),
        "webview-made-again" => WebviewMadeAgain { window: "overlay", took: ms(850) }.to_string(),
        "hotkey-pressed" => HotkeyPressed { id: 1, action: &Some(Action::Review) }.to_string(),
        other => panic!("tests/contracts/log-lines.json lists {other}, which these tests don't write"),
    }
}

#[test]
fn each_line_is_written_as_the_fixture_says() {
    let mut ids = BTreeSet::new();
    for line in lines() {
        let id = text(&line, "id");
        assert!(ids.insert(id.to_string()), "{id} is listed twice");
        assert_eq!(written(id), text(&line, "text"), "{id}: update tests/contracts/log-lines.json and the drivers");
    }
}

/// A driver's pattern with its variables replaced by this line's values.
fn pattern(matcher: &Value) -> String {
    let mut pattern = text(matcher, "pattern").to_string();
    if let Some(vars) = matcher["vars"].as_object() {
        for (name, value) in vars {
            assert!(pattern.contains(name.as_str()), "{name} isn't in {pattern}");
            pattern = pattern.replace(name.as_str(), value.as_str().unwrap());
        }
    }
    pattern
}

#[test]
fn each_driver_pattern_matches_its_line() {
    let fixture = fixture();
    for line in lines() {
        let (id, written) = (text(&line, "id"), text(&line, "text"));
        let matchers = line["matched_by"].as_array().unwrap();
        assert!(!matchers.is_empty(), "{id}: no driver matches it, so it needn't be listed");
        for matcher in matchers {
            let driver = text(matcher, "driver");
            let stamp = text(&fixture["drivers"][driver], "timestamp");
            let full = format!("{stamp} {written}");
            let pattern = pattern(matcher);
            let matches = match text(matcher, "kind") {
                "substring" => full.contains(&pattern),
                "regex" => Regex::new(&pattern).unwrap().is_match(&full),
                "regex-ignore-case" => Regex::new(&format!("(?i){pattern}")).unwrap().is_match(&full),
                other => panic!("{id}: unknown kind {other}"),
            };
            assert!(matches, "{id}: {driver}'s {pattern:?} doesn't match {full:?}");
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
    assert!(has_whole_literal("if ($line -match ': applied in ') {", ": applied in "));
    assert!(!has_whole_literal("if ($line -match ': applied in $') {", ": applied in "));
    assert!(!has_whole_literal("grep 'replace foot: applied in '", ": applied in "));
}

#[test]
fn each_driver_pattern_is_in_the_driver() {
    for line in lines() {
        for matcher in line["matched_by"].as_array().unwrap() {
            let file = text(matcher, "file");
            assert!(file.starts_with("tests/ui/") || file.starts_with("scripts/ui-smoke"), "{file} isn't a driver");
            let source = std::fs::read_to_string(repository().join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
            let pattern = text(matcher, "pattern");
            assert!(
                has_whole_literal(&source, pattern),
                "{}: {file} no longer has {pattern:?} as a whole literal; update tests/contracts/log-lines.json to what it matches now",
                text(&line, "id")
            );
        }
    }
}

/// Every type here has a line in the fixture, so none is left out of the check.
#[test]
fn each_type_has_a_line_in_the_fixture() {
    let source = include_str!("../log_lines.rs");
    let types: BTreeSet<&str> = source
        .lines()
        .filter_map(|l| l.strip_prefix("pub struct "))
        .map(|rest| rest.split(|c: char| !c.is_alphanumeric()).next().unwrap())
        .collect();
    let lines = lines();
    let writers: BTreeSet<&str> = lines.iter().flat_map(|line| text(line, "writer").split(" + ")).collect();
    let missing: Vec<_> = types.difference(&writers).collect();
    assert!(missing.is_empty(), "no line in tests/contracts/log-lines.json for {missing:?}");
}
