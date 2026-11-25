use super::*;
use crate::provider::ProviderKind;
use serde_json::json;

#[test]
fn only_an_absent_file_is_a_first_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let missing = ConfigFile::load(&path);
    assert_eq!(missing.status(), LoadStatus::Missing);
    assert!(missing.exclusions_safe && missing.destination_safe);

    for bytes in [b"".as_slice(), b"{broken", b"null", b"[]"] {
        fs::write(&path, bytes).unwrap();
        let mut loaded = ConfigFile::load(&path);
        assert_eq!(loaded.status(), LoadStatus::Unreadable);
        assert!(!loaded.exclusions_safe && !loaded.destination_safe);
        assert!(loaded.save(&path, Config::default()).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(dir.path().read_dir().unwrap().count(), 1);
    }
}

#[cfg(unix)]
#[test]
fn denied_reads_block_writes_until_the_file_is_readable_again() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, br#"{"words":["QuillPoint"]}"#).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let mut loaded = ConfigFile::load(&path);
    // Root can still read a file with no permissions.
    if fs::read(&path).is_err() {
        assert_eq!(loaded.status(), LoadStatus::Unreadable);
        assert!(loaded.save(&path, Config::default()).is_err());
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let loaded = ConfigFile::load(&path);
    assert_eq!(loaded.status(), LoadStatus::Clean);
    assert_eq!(loaded.config.words, ["QuillPoint"]);
}

#[cfg(unix)]
#[test]
fn a_failed_repair_backup_leaves_the_file_and_revision_source_intact() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let bytes = br#"{"auto_min_chars": false, "words": ["QuillPoint"]}"#;
    fs::write(&path, bytes).unwrap();
    let mut loaded = ConfigFile::load(&path);
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let writable = tempfile::NamedTempFile::new_in(dir.path()).is_ok();
    let mut edited = loaded.config.clone();
    edited.words.push("Brynlee".into());
    let saved = loaded.save(&path, edited.clone());
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    if !writable {
        assert!(saved.is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(loaded.config.words, ["QuillPoint"]);
        assert_eq!(dir.path().read_dir().unwrap().count(), 1);
        loaded.save(&path, edited).unwrap();
    } else {
        saved.unwrap();
    }
    assert_eq!(loaded.config.words, ["QuillPoint", "Brynlee"]);
}

#[test]
fn recovers_fields_and_keeps_future_json_and_providers_through_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut provider = serde_json::to_value(ProviderProfile::new(ProviderKind::Ollama)).unwrap();
    provider["future_options"] = json!({"preset": "Brynlee"});
    let future = json!({"id": "quillpoint", "kind": "future_ai", "destination": {"region": 42}});
    let raw = json!({
        "future_setting": {"enabled": true},
        "excluded_apps": ["com.example.quillpoint"],
        "auto_min_chars": "broken",
        (HOTKEYS_KEY): {"quick_fix": "Super+Alt+P", "review": false, "future_key": ["R"]},
        "providers": [future, provider],
        "active_provider": "ollama",
    });
    let bytes = serde_json::to_vec(&raw).unwrap();
    fs::write(&path, &bytes).unwrap();
    let mut loaded = ConfigFile::load(&path);
    assert_eq!(loaded.status(), LoadStatus::Recovered);
    assert!(loaded.exclusions_safe && loaded.destination_safe);
    assert!(loaded.config.is_excluded("com.example.quillpoint"));
    assert_eq!(loaded.config.auto_min_chars, Config::default().auto_min_chars);
    assert_eq!(loaded.config.hotkeys.quick_fix, "Super+Alt+P");
    assert_eq!(loaded.config.active_profile().unwrap().id, "ollama");
    assert!(loaded.unavailable_provider_ids().contains("quillpoint"));

    let mut edited = loaded.config.clone();
    edited.providers[0].model = "new-model".into();
    edited.words.push("QuillPoint".into());
    loaded.save(&path, edited).unwrap();
    loaded.save(&path, loaded.config.clone()).unwrap();
    let backup: Vec<_> =
        dir.path().read_dir().unwrap().flatten().map(|entry| entry.path()).filter(|entry| entry != &path).collect();
    assert_eq!(backup.len(), 1);
    assert_eq!(fs::read(&backup[0]).unwrap(), bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&backup[0]).unwrap().permissions().mode() & 0o777, 0o600);
    }

    let repaired: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(repaired["future_setting"], raw["future_setting"]);
    assert_eq!(repaired["providers"][0]["future_options"], provider["future_options"]);
    assert_eq!(repaired["providers"][1], future);
    assert_eq!(repaired[HOTKEYS_KEY]["future_key"], raw[HOTKEYS_KEY]["future_key"]);
    assert_eq!(ConfigFile::load(&path).config.words, ["QuillPoint"]);
    let mut edited = loaded.config.clone();
    edited.providers.clear();
    edited.active_provider = None;
    loaded.save(&path, edited).unwrap();
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["providers"], json!([future]), "only the deliberately removed profile disappears");
}

#[test]
fn an_unrelated_save_preserves_unsafe_values_and_recovery_fences() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let raw = json!({
        "auto_popup": true,
        "excluded_apps": ["com.example.quillpoint", false],
        "active_provider": {"type": "future_ai"},
    });
    fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
    let mut loaded = ConfigFile::load(&path);
    assert!(!loaded.exclusions_safe && !loaded.destination_safe);
    assert!(loaded.config.is_excluded("com.example.quillpoint"));
    let mut edited = loaded.config.clone();
    edited.style_notes = "Short sentences.".into();
    loaded.save(&path, edited).unwrap();
    let loaded = ConfigFile::load(&path);
    assert!(!loaded.exclusions_safe && !loaded.destination_safe);
    assert_eq!(loaded.config.style_notes, "Short sentences.");
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["excluded_apps"], raw["excluded_apps"]);
    assert_eq!(saved["active_provider"], raw["active_provider"]);
}

#[test]
fn an_unknown_or_ambiguous_active_profile_stays_inactive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let profile = ProviderProfile::new(ProviderKind::Ollama);
    for providers in [
        json!([{ "id": "ollama", "kind": "future_ai" }]),
        json!([profile, { "id": "ollama", "kind": "future_ai" }]),
        json!([]),
    ] {
        let raw = json!({"active_provider": "ollama", "providers": providers});
        fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        let loaded = ConfigFile::load(&path);
        assert!(!loaded.destination_safe);
        assert!(loaded.config.active_profile().is_none());
        assert!(loaded.unavailable_provider_ids().contains("ollama"));
    }
}

#[test]
fn an_external_replacement_is_kept_until_an_explicit_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let original = br#"{"words":["Rhyan"]}"#;
    let external = br#"{"words":["Brynlee"]}"#;
    fs::write(&path, original).unwrap();
    let mut loaded = ConfigFile::load(&path);
    fs::write(&path, external).unwrap();
    assert!(loaded.save(&path, loaded.config.clone()).is_err());
    assert_eq!(fs::read(&path).unwrap(), external);
    assert_eq!(ConfigFile::load(&path).config.words, ["Brynlee"]);
}
