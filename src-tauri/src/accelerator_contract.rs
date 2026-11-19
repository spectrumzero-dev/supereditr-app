//! The accelerator strings Settings records, read by every Rust parser that takes them.
//!
//! tests/contracts/accelerators.json lists key presses and the string `acceleratorFromEvent`
//! (ui/src/lib/accelerator.ts) records for each; ui/src/lib/contracts.test.ts checks that side.
//! These tests check that each string reads back as the same key and modifiers here:
//! `shortcut_check::parse` (macOS key codes), `double_press::parse` (presses twice) and
//! `keys::chord` (Windows virtual keys), and on macOS the global-shortcut plugin that registers
//! single presses. `windowsKeyNames` pins `keys::name` to `windowsKeyName`.

use crate::double_press::{self, ALT, COMMAND, CONTROL, SHIFT};
use crate::keys;
use crate::shortcut_check::{self, Combo, Parsed};
use serde_json::Value;

fn fixture() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/contracts/accelerators.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn modifiers(names: &Value) -> u8 {
    names.as_array().unwrap().iter().fold(0, |bits, name| {
        bits | match name.as_str().unwrap() {
            "Command" => COMMAND,
            "Control" => CONTROL,
            "Alt" => ALT,
            "Shift" => SHIFT,
            other => panic!("unknown modifier {other}"),
        }
    })
}

/// A key code: a number, or a hex string such as "0xBA"; `None` for null.
fn code(value: &Value) -> Option<u16> {
    if value.is_null() {
        return None;
    }
    Some(match value.as_str() {
        Some(hex) => u16::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap(),
        None => u16::try_from(value.as_u64().unwrap()).unwrap(),
    })
}

/// What the parsers every platform compiles read from `accelerator`.
fn check_parsers(name: &str, accelerator: &str, parsed: &Value, double: bool) {
    let modifiers = modifiers(&parsed["modifiers"]);
    assert_eq!(
        shortcut_check::parse(accelerator),
        code(&parsed["macKeycode"]).map(|keycode| Parsed { combo: Combo { modifiers, keycode }, double }),
        "{name}: shortcut_check::parse({accelerator:?})"
    );
    assert_eq!(
        keys::chord(accelerator),
        code(&parsed["windowsVk"]).map(|vk| keys::Chord { modifiers, vk }),
        "{name}: keys::chord({accelerator:?})"
    );
    if double {
        let key = parsed["doubleKey"].as_str().unwrap().to_string();
        assert_eq!(
            double_press::parse(accelerator),
            Some(Ok(double_press::Chord { modifiers, key })),
            "{name}: double_press::parse({accelerator:?})"
        );
    } else {
        assert_eq!(double_press::parse(accelerator), None, "{name}: a single press isn't a double press");
        let twice = format!("{accelerator} {accelerator}");
        assert_eq!(
            matches!(double_press::parse(&twice), Some(Ok(_))),
            parsed["canDouble"].as_bool().unwrap(),
            "{name}: double_press::parse({twice:?})"
        );
    }
}

#[test]
fn recorded_accelerators_parse_as_recorded() {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        // A key the recorder refuses leaves nothing to parse.
        let Some(accelerator) = case["accelerator"].as_str() else { continue };
        check_parsers(case["name"].as_str().unwrap(), accelerator, &case["parsed"], false);
    }
}

#[test]
fn recorded_double_presses_parse_as_recorded() {
    let fixture = fixture();
    let cases = fixture["doubled"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        check_parsers(case["name"].as_str().unwrap(), case["accelerator"].as_str().unwrap(), &case["parsed"], true);
    }
}

/// macOS registers a single press with the global-shortcut plugin (`hotkeys::register_from_config`),
/// which must accept every string the macOS recorder writes.
#[cfg(target_os = "macos")]
#[test]
fn the_global_shortcut_plugin_accepts_every_macos_recording() {
    use std::str::FromStr;
    use tauri_plugin_global_shortcut::Shortcut;
    let fixture = fixture();
    for case in fixture["cases"].as_array().unwrap() {
        let on_macos = case["platforms"].as_array().unwrap().iter().any(|p| p == "macos");
        let Some(accelerator) = case["accelerator"].as_str().filter(|_| on_macos) else { continue };
        assert!(Shortcut::from_str(accelerator).is_ok(), "{accelerator:?}");
    }
}

/// `keys::name` and `windowsKeyName` agree on these keys. They differ on keys the recorder doesn't
/// name: `keys::name` gives Esc, Backspace, Delete, PrintScreen and Pause their names and any other
/// key `VkXX`, where `windowsKeyName` returns null.
#[test]
fn windows_key_names_match_the_recorder() {
    let fixture = fixture();
    let names = fixture["windowsKeyNames"].as_array().unwrap();
    assert!(!names.is_empty());
    for entry in names {
        let vk = code(&entry["vk"]).unwrap();
        let name = entry["name"].as_str().unwrap();
        assert_eq!(keys::name(vk), name, "keys::name({vk:#x})");
        assert_eq!(keys::vk(name), Some(vk), "keys::vk({name:?})");
    }
}
