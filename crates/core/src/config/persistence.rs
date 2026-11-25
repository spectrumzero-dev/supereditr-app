use super::{Config, HOTKEYS_KEY};
use crate::prompts::Preset;
use crate::provider::ProviderProfile;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

type Fields = Map<String, Value>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadStatus {
    Missing,
    Clean,
    Recovered,
    Unreadable,
}

/// The settings in use and their on-disk source. Keep both under the same commit lock so a
/// save can detect an external change and preserve data this version doesn't understand.
pub struct ConfigFile {
    pub config: Config,
    pub issues: Vec<&'static str>,
    pub exclusions_safe: bool,
    pub destination_safe: bool,
    source: Source,
    backed_up: bool,
}

enum Source {
    Missing,
    Document { bytes: Vec<u8>, fields: Fields },
    Unreadable,
}

impl ConfigFile {
    pub fn load(path: &Path) -> Self {
        match read(path) {
            Ok(None) => Self {
                config: Config::default(),
                issues: Vec::new(),
                exclusions_safe: true,
                destination_safe: true,
                source: Source::Missing,
                backed_up: false,
            },
            Ok(Some(bytes)) => match serde_json::from_slice::<Fields>(&bytes) {
                Ok(fields) => Self::from_document(bytes, fields),
                Err(_) => Self::unavailable("Settings must contain a valid JSON object."),
            },
            Err(_) => Self::unavailable("SuperEditr couldn’t read the settings file."),
        }
    }

    pub fn unavailable(issue: &'static str) -> Self {
        Self {
            config: Config::default(),
            issues: vec![issue],
            exclusions_safe: false,
            destination_safe: false,
            source: Source::Unreadable,
            backed_up: false,
        }
    }

    pub fn status(&self) -> LoadStatus {
        match self.source {
            Source::Missing => LoadStatus::Missing,
            Source::Unreadable => LoadStatus::Unreadable,
            Source::Document { .. } if self.issues.is_empty() => LoadStatus::Clean,
            Source::Document { .. } => LoadStatus::Recovered,
        }
    }

    /// IDs whose provider records exist in the file but couldn't be read.
    pub fn unavailable_provider_ids(&self) -> HashSet<&str> {
        let Source::Document { fields, .. } = &self.source else { return HashSet::new() };
        let usable = record_ids::<ProviderProfile>(fields.get("providers"));
        entries(fields.get("providers")).iter().filter_map(id).filter(|id| !usable.contains(id)).collect()
    }

    /// Whether absence from the readable and unreadable IDs means a profile was removed.
    pub fn provider_ids_complete(&self) -> bool {
        self.providers_editable()
            && match &self.source {
                Source::Document { fields, .. } => {
                    entries(fields.get("providers")).iter().all(|entry| id(entry).is_some())
                }
                _ => true,
            }
    }

    /// An unresolved active choice also reserves its ID, so a new profile can't take it over.
    pub fn reserved_provider_ids(&self) -> HashSet<&str> {
        let mut unavailable = self.unavailable_provider_ids();
        if !self.destination_safe
            && let Some(active) = self.config.active_provider.as_deref()
        {
            unavailable.insert(active);
        }
        unavailable
    }

    pub fn providers_editable(&self) -> bool {
        match &self.source {
            Source::Missing => true,
            Source::Unreadable => false,
            Source::Document { fields, .. } => fields.get("providers").is_none_or(Value::is_array),
        }
    }

    pub fn save(&mut self, path: &Path, mut config: Config) -> io::Result<()> {
        let (expected, fields) = match &self.source {
            Source::Missing => (None, Fields::new()),
            Source::Unreadable => return Err(io::Error::other("Repair the settings file and retry loading it.")),
            Source::Document { bytes, fields } => (Some(bytes.as_slice()), fields.clone()),
        };
        let unchanged = || -> io::Result<()> {
            if read(path)?.as_deref() != expected {
                return Err(io::Error::other("The settings file changed on disk. Retry to load the current file."));
            }
            Ok(())
        };
        unchanged()?;
        if !self.exclusions_safe
            && (config.excluded_apps != self.config.excluded_apps
                || config.offered_exclusions != self.config.offered_exclusions)
        {
            return Err(io::Error::other("Repair the app exclusions in the settings file and retry loading it."));
        }
        if !self.providers_editable() && config.providers != self.config.providers {
            return Err(io::Error::other("Repair the providers in the settings file and retry loading it."));
        }
        if self.issues.contains(&"words") && config.words != self.config.words {
            return Err(io::Error::other("Repair the dictionary in the settings file and retry loading it."));
        }
        let reserved_ids = self.reserved_provider_ids();
        if config.providers.iter().any(|provider| reserved_ids.contains(provider.id.as_str())) {
            return Err(io::Error::other("This provider ID belongs to an unavailable provider."));
        }

        config.pin_official_addresses();
        let mut fields = render(fields, &config, self)?;
        // Choices the user hasn't changed may still refer to an unsupported or damaged record.
        // Keep the exact value in that case, including a future non-string representation.
        if !self.destination_safe
            && config.active_provider == self.config.active_provider
            && let Source::Document { fields: original, .. } = &self.source
            && let Some(active) = original.get("active_provider")
        {
            fields.insert("active_provider".into(), active.clone());
        }
        let bytes = serde_json::to_vec_pretty(&fields).map_err(io::Error::other)?;
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        if !self.backed_up
            && self.status() == LoadStatus::Recovered
            && let Some(original) = expected
        {
            // A uniquely named backup avoids replacing an earlier repair's original. Temporary
            // files use owner-only mode on Unix and the per-user config directory's ACL on Windows.
            let mut backup = tempfile::Builder::new().prefix("settings-backup-").suffix(".json").tempfile_in(parent)?;
            backup.write_all(original)?;
            backup.as_file().sync_all()?;
            backup.keep().map_err(|e| e.error)?;
            sync_directory(parent)?;
            self.backed_up = true;
        }
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(&bytes)?;
        temp.as_file().sync_all()?;
        unchanged()?;
        temp.persist(path).map_err(|e| e.error)?;
        // The replacement is already visible. Keep the in-memory state in step with it even on
        // filesystems that can't confirm the directory entry was flushed.
        if let Err(e) = sync_directory(parent) {
            eprintln!("SuperEditr: couldn’t sync the settings directory: {e}");
        }
        let backed_up = self.backed_up;
        *self = Self::from_document(bytes, fields);
        self.config = config;
        self.backed_up = backed_up;
        Ok(())
    }

    fn from_document(bytes: Vec<u8>, fields: Fields) -> Self {
        let (config, mut issues) = recover(&fields);
        let exclusions_safe = !issues.contains(&"excluded_apps") && !issues.contains(&"offered_exclusions");
        let destination_safe = !issues.contains(&"active_provider")
            && (config.active_provider.is_none() || config.active_profile().is_some());
        if !destination_safe && !issues.contains(&"active_provider") {
            issues.push("active_provider");
        }
        Self {
            config,
            issues,
            exclusions_safe,
            destination_safe,
            source: Source::Document { bytes, fields },
            backed_up: false,
        }
    }
}

/// A missing target of an existing link is a read failure, even though opening it gave NotFound.
fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => match fs::symlink_metadata(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            _ => Err(e),
        },
        Err(e) => Err(e),
    }
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn read_field<T: DeserializeOwned>(fields: &Fields, key: &'static str, into: &mut T, issues: &mut Vec<&'static str>) {
    if let Some(value) = fields.get(key) {
        match serde_json::from_value(value.clone()) {
            Ok(value) => *into = value,
            Err(_) => issues.push(key),
        }
    }
}

fn recover(fields: &Fields) -> (Config, Vec<&'static str>) {
    let mut config = Config {
        offered_presets: Vec::new(),
        offered_exclusions: super::first_default_exclusions(),
        ..Config::default()
    };
    let mut issues = Vec::new();
    macro_rules! fields {
        ($($field:ident),*) => {
            $(read_field(fields, stringify!($field), &mut config.$field, &mut issues);)*
        };
    }
    fields!(
        active_provider,
        offered_presets,
        default_preset,
        style_notes,
        dialect,
        auto_popup,
        auto_min_chars,
        auto_max_chars,
        select_sentence,
        keep_formatting,
        words,
        excluded_apps,
        offered_exclusions,
        show_timing,
        always_show_tips,
        diagnostic_log,
        offered_launch_at_login,
        onboarded
    );
    if issues.contains(&"excluded_apps")
        && let Some(excluded) = fields.get("excluded_apps").and_then(Value::as_array)
    {
        for app in excluded.iter().filter_map(Value::as_str) {
            config.set_excluded(app, true);
        }
    }
    if issues.contains(&"words")
        && let Some(words) = fields.get("words").and_then(Value::as_array)
    {
        config.words = words.iter().filter_map(Value::as_str).map(str::to_owned).collect();
    }
    if let Some(raw) = fields.get(HOTKEYS_KEY) {
        if let Some(hotkeys) = raw.as_object() {
            let mut shortcut_issues = Vec::new();
            read_field(hotkeys, "review", &mut config.hotkeys.review, &mut shortcut_issues);
            read_field(hotkeys, "quick_fix", &mut config.hotkeys.quick_fix, &mut shortcut_issues);
            if !shortcut_issues.is_empty() {
                issues.push(HOTKEYS_KEY);
            }
        } else {
            issues.push(HOTKEYS_KEY);
        }
    }
    read_records(fields, "providers", &mut config.providers, &mut issues);
    read_records(fields, "presets", &mut config.presets, &mut issues);
    config.migrate();
    (config, issues)
}

fn entries(value: Option<&Value>) -> &[Value] {
    value.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default()
}

fn id(value: &Value) -> Option<&str> {
    value.get("id")?.as_str().filter(|id| !id.trim().is_empty())
}

/// Ambiguous IDs are unavailable, even when one of the records is otherwise readable.
fn record_ids<T: DeserializeOwned>(raw: Option<&Value>) -> HashSet<&str> {
    let mut counts = HashMap::new();
    for id in entries(raw).iter().filter_map(id) {
        *counts.entry(id).or_insert(0) += 1;
    }
    entries(raw)
        .iter()
        .filter_map(|entry| {
            let id = id(entry)?;
            (counts[id] == 1 && serde_json::from_value::<T>(entry.clone()).is_ok()).then_some(id)
        })
        .collect()
}

fn read_records<T: DeserializeOwned>(
    fields: &Fields,
    key: &'static str,
    into: &mut Vec<T>,
    issues: &mut Vec<&'static str>,
) {
    let Some(raw) = fields.get(key) else { return };
    let usable = record_ids::<T>(Some(raw));
    if !raw.is_array() || usable.len() != entries(Some(raw)).len() {
        issues.push(key);
    }
    *into = entries(Some(raw))
        .iter()
        .filter(|entry| id(entry).is_some_and(|id| usable.contains(id)))
        .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
        .collect();
}

fn render(mut fields: Fields, config: &Config, original: &ConfigFile) -> io::Result<Fields> {
    let Value::Object(known) = serde_json::to_value(config).map_err(io::Error::other)? else { unreachable!() };
    for (key, mut value) in known {
        let key = if key == "hotkeys" { HOTKEYS_KEY.to_owned() } else { key };
        let previous = fields.get(&key);
        match key.as_str() {
            HOTKEYS_KEY => value = merge_object(previous, value, &[]),
            "providers" if !original.providers_editable() => continue,
            "providers" => value = merge_records::<ProviderProfile>(previous, value, &["extra_body", "api_key"], true),
            "presets" => value = merge_records::<Preset>(previous, value, &["shortcut"], false),
            "excluded_apps" | "offered_exclusions" | "words" if original.issues.contains(&key.as_str()) => continue,
            _ => {}
        }
        fields.insert(key, value);
    }
    Ok(fields)
}

fn merge_object(previous: Option<&Value>, value: Value, remove: &[&str]) -> Value {
    let Some(mut previous) = previous.and_then(Value::as_object).cloned() else { return value };
    let Value::Object(known) = value else { return value };
    for key in remove {
        previous.remove(*key);
    }
    previous.extend(known);
    Value::Object(previous)
}

fn merge_records<T: DeserializeOwned>(
    previous: Option<&Value>,
    value: Value,
    remove: &[&str],
    same_kind: bool,
) -> Value {
    let readable = record_ids::<T>(previous);
    let mut by_id: HashMap<_, _> =
        entries(previous).iter().filter_map(|entry| id(entry).map(|id| (id, entry))).collect();
    let Value::Array(known) = value else { unreachable!() };
    let mut records: Vec<_> = known
        .into_iter()
        .map(|entry| {
            let old = id(&entry).filter(|id| readable.contains(id)).and_then(|id| by_id.remove(id));
            let old = old.filter(|old| !same_kind || old.get("kind") == entry.get("kind"));
            merge_object(old, entry, remove)
        })
        .collect();
    let retained: HashSet<_> = records.iter().filter_map(id).collect();
    let unavailable: Vec<_> = entries(previous)
        .iter()
        .filter(|entry| id(entry).is_none_or(|id| !readable.contains(id) && !retained.contains(id)))
        .cloned()
        .collect();
    records.extend(unavailable);
    Value::Array(records)
}

#[cfg(test)]
mod tests;
