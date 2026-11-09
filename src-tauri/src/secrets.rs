//! API keys live in the OS credential store (macOS Keychain, Windows Credential Manager, and on
//! Linux the Secret Service, such as GNOME Keyring).
//!
//! Each key is stored under its provider's kind as well as its profile ID, so a key is only ever
//! read back for the kind of service it was saved for.

use supereditr_core::provider::ProviderKind;

const SERVICE: &str = "app.supereditr";

#[cfg(target_os = "macos")]
const STORE: &str = "keychain";
#[cfg(not(target_os = "macos"))]
const STORE: &str = "keyring";

/// The credential store's account name for a provider's key: its kind and ID, as in
/// `anthropic:anthropic` or `open_ai_compatible:compatible-2`.
pub fn account(kind: ProviderKind, profile_id: &str) -> String {
    let kind = serde_json::to_value(kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    format!("{kind}:{profile_id}")
}

/// Reads the key saved under an `account` name.
pub fn get(account: &str) -> Option<String> {
    keyring::Entry::new(SERVICE, account).ok()?.get_password().ok().filter(|k| !k.is_empty())
}

pub fn set(kind: ProviderKind, profile_id: &str, key: &str) -> Result<(), String> {
    keyring::Entry::new(SERVICE, &account(kind, profile_id))
        .and_then(|entry| entry.set_password(key))
        .map_err(|e| format!("Couldn’t save the key to the {STORE}: {e}"))
}

pub fn delete(kind: ProviderKind, profile_id: &str) {
    if let Ok(entry) = keyring::Entry::new(SERVICE, &account(kind, profile_id)) {
        let _ = entry.delete_credential();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_stored_under_its_kind_and_id() {
        assert_eq!(account(ProviderKind::Anthropic, "anthropic"), "anthropic:anthropic");
        assert_eq!(account(ProviderKind::OpenAiCompatible, "compatible-2"), "open_ai_compatible:compatible-2");
        // The same ID under another kind is another account, so it finds no key.
        assert_ne!(account(ProviderKind::Anthropic, "anthropic"), account(ProviderKind::OpenAiCompatible, "anthropic"));
    }
}
