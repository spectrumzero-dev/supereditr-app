//! API keys live in the OS credential store (macOS Keychain, Windows Credential Manager).

const SERVICE: &str = "com.sghael.supereditr";

pub fn get(profile_id: &str) -> Option<String> {
    keyring::Entry::new(SERVICE, profile_id).ok()?.get_password().ok().filter(|k| !k.is_empty())
}

pub fn set(profile_id: &str, key: &str) -> Result<(), String> {
    keyring::Entry::new(SERVICE, profile_id)
        .and_then(|entry| entry.set_password(key))
        .map_err(|e| format!("Couldn’t save the key to the keychain: {e}"))
}

pub fn delete(profile_id: &str) {
    if let Ok(entry) = keyring::Entry::new(SERVICE, profile_id) {
        let _ = entry.delete_credential();
    }
}
