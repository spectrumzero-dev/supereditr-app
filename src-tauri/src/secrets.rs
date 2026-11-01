//! API keys live in the OS credential store (macOS Keychain, Windows Credential Manager).

const SERVICE: &str = "app.supereditr";
/// Where versions before 0.1.2 kept keys. `get` copies a key from here the first time it reads it,
/// and leaves it in place, as the old settings folder is.
const OLD_SERVICE: &str = crate::OLD_IDENTIFIER;

pub fn get(profile_id: &str) -> Option<String> {
    read(SERVICE, profile_id).or_else(|| {
        let key = read(OLD_SERVICE, profile_id)?;
        if let Err(e) = set(profile_id, &key) {
            eprintln!("SuperEditr: couldn't copy a key saved before 0.1.2: {e}");
        }
        Some(key)
    })
}

fn read(service: &str, profile_id: &str) -> Option<String> {
    keyring::Entry::new(service, profile_id).ok()?.get_password().ok().filter(|k| !k.is_empty())
}

pub fn set(profile_id: &str, key: &str) -> Result<(), String> {
    keyring::Entry::new(SERVICE, profile_id)
        .and_then(|entry| entry.set_password(key))
        .map_err(|e| format!("Couldn’t save the key to the keychain: {e}"))
}

/// Removes the key under both names, so the copy from before 0.1.2 can't bring it back.
pub fn delete(profile_id: &str) {
    forget(SERVICE, profile_id);
    forget(OLD_SERVICE, profile_id);
}

fn forget(service: &str, profile_id: &str) {
    if let Ok(entry) = keyring::Entry::new(service, profile_id) {
        let _ = entry.delete_credential();
    }
}
