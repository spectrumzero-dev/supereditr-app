//! API keys live in the OS credential store (macOS Keychain, Windows Credential Manager).

const SERVICE: &str = "app.supereditr";
/// Where versions before 0.1.2 kept keys.
const OLD_SERVICE: &str = crate::OLD_IDENTIFIER;

pub fn get(profile_id: &str) -> Option<String> {
    read(SERVICE, profile_id)
}

fn read(service: &str, profile_id: &str) -> Option<String> {
    keyring::Entry::new(service, profile_id).ok()?.get_password().ok().filter(|k| !k.is_empty())
}

/// Copies the keys these providers had before 0.1.2 to the current service, leaving the old ones
/// in place, as the old settings folder is. Runs once, when the settings are copied, so a key the
/// user removes later can't come back from the old service.
pub fn copy_old<'a>(profile_ids: impl IntoIterator<Item = &'a str>) {
    for profile_id in profile_ids {
        if read(SERVICE, profile_id).is_none()
            && let Some(key) = read(OLD_SERVICE, profile_id)
            && let Err(e) = set(profile_id, &key)
        {
            eprintln!("SuperEditr: couldn't copy a key saved before 0.1.2: {e}");
        }
    }
}

pub fn set(profile_id: &str, key: &str) -> Result<(), String> {
    keyring::Entry::new(SERVICE, profile_id)
        .and_then(|entry| entry.set_password(key))
        .map_err(|e| format!("Couldn’t save the key to the keychain: {e}"))
}

/// Removes the key, and the copy from before 0.1.2 if there is one.
pub fn delete(profile_id: &str) {
    forget(SERVICE, profile_id);
    forget(OLD_SERVICE, profile_id);
}

fn forget(service: &str, profile_id: &str) {
    if let Ok(entry) = keyring::Entry::new(service, profile_id) {
        let _ = entry.delete_credential();
    }
}
