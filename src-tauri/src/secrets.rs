//! API keys live in the OS credential store (macOS Keychain, Windows Credential Manager, and on
//! Linux the Secret Service, such as GNOME Keyring).
//!
//! Each key is stored under its provider's kind as well as its profile ID, so a key is only ever
//! read back for the kind of service it was saved for.

use supereditr_core::provider::ProviderKind;

const SERVICE: &str = "app.supereditr";
/// Where versions before 0.1.2 kept keys. Only ever deleted from, never read.
const OLD_SERVICE: &str = crate::OLD_IDENTIFIER;

#[cfg(target_os = "macos")]
const STORE: &str = "keychain";
#[cfg(not(target_os = "macos"))]
const STORE: &str = "keyring";

/// Whether this provider can have a saved credential.
pub fn can_store_key(kind: ProviderKind) -> bool {
    kind.needs_key() || kind == ProviderKind::OpenAiCompatible
}

/// The credential store's account name for a provider's key: its kind and ID, as in
/// `anthropic:anthropic` or `open_ai_compatible:compatible-2`.
pub fn account(kind: ProviderKind, profile_id: &str) -> String {
    let kind = serde_json::to_value(kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    format!("{kind}:{profile_id}")
}

/// Where API keys are saved: the OS credential store in the app ([`CredentialStore`]), and a map in
/// unit tests. Reading a key goes through the app state's credential cache.
pub trait Secrets: Sync {
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    fn set(&self, kind: ProviderKind, profile_id: &str, key: &str) -> Result<(), String>;
    /// Returns true for a confirmed deletion, false when the entry was already absent.
    fn delete_entry(&self, service: &str, account: &str) -> Result<bool, String>;

    /// Removes the key and any copies earlier versions kept for this profile. Try every
    /// entry even after a failure, so a retry only has to clean up what remains.
    fn delete(&self, kind: ProviderKind, profile_id: &str) -> Result<usize, String> {
        let mut deleted = 0;
        let mut failures = Vec::new();
        for (service, account) in entries_to_delete(kind, profile_id) {
            match self.delete_entry(service, &account) {
                Ok(true) => deleted += 1,
                Ok(false) => (),
                Err(error) => failures.push(error),
            }
        }
        if failures.is_empty() {
            Ok(deleted)
        } else {
            Err(format!(
                "Couldn’t confirm key cleanup for {profile_id}. Deleted {deleted} credential entries. {}",
                failures.join(" ")
            ))
        }
    }
}

/// The OS credential store.
pub struct CredentialStore;

impl Secrets for CredentialStore {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        // Unit tests only use fake credentials or keys already cached in AppState.
        if cfg!(test) {
            return Ok(None);
        }
        match keyring::Entry::new(SERVICE, account).and_then(|entry| entry.get_password()) {
            Ok(key) => Ok(Some(key).filter(|key| !key.is_empty())),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(format!("Couldn’t read the key from the {STORE}: {error}")),
        }
    }

    fn set(&self, kind: ProviderKind, profile_id: &str, key: &str) -> Result<(), String> {
        keyring::Entry::new(SERVICE, &account(kind, profile_id))
            .and_then(|entry| entry.set_password(key))
            .map_err(|e| format!("Couldn’t save the key to the {STORE}: {e}"))
    }

    fn delete_entry(&self, service: &str, account: &str) -> Result<bool, String> {
        let failure = |error| format!("Couldn’t delete a key from the {STORE}: {error}");
        let entry = keyring::Entry::new(service, account).map_err(failure)?;
        match entry.delete_credential() {
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(error) => Err(failure(error)),
            Ok(()) => {
                // The macOS backend discards the OS status from the delete itself.
                #[cfg(target_os = "macos")]
                match entry.get_secret() {
                    Err(keyring::Error::NoEntry) => (),
                    Ok(_) => return Err("A key is still in the keychain after deletion.".into()),
                    Err(error) => return Err(format!("Couldn’t verify deletion from the {STORE}: {error}")),
                }
                Ok(true)
            }
        }
    }
}

fn entries_to_delete(kind: ProviderKind, profile_id: &str) -> [(&'static str, String); 3] {
    [(SERVICE, account(kind, profile_id)), (SERVICE, profile_id.to_string()), (OLD_SERVICE, profile_id.to_string())]
}

/// Whether a Credential Manager entry holds a key SuperEditr saved. The keyring crate names it
/// "<account>.app.supereditr" and puts the account name (see [`account`]) in its user name;
/// versions before 0.1.2 saved under their own service ("<profile ID>.com.sghael.supereditr").
#[cfg_attr(not(windows), allow(dead_code))]
fn is_saved_key(target: &str, user: &str) -> bool {
    [SERVICE, OLD_SERVICE]
        .iter()
        .any(|service| target.strip_suffix(service).and_then(|rest| rest.strip_suffix('.')) == Some(user))
}

/// Deletes every key SuperEditr saved in Credential Manager, including any left by a provider
/// that's gone, and nothing else, for "Delete app data" in the uninstaller. Returns how many it
/// deleted.
#[cfg(windows)]
pub fn delete_all() -> Result<usize, String> {
    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{
        CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredEnumerateW, CredFree,
    };
    use windows::core::PCWSTR;
    let mut count = 0u32;
    let mut list: *mut *mut CREDENTIALW = std::ptr::null_mut();
    match unsafe { CredEnumerateW(PCWSTR::null(), None, &mut count, &mut list) } {
        Ok(()) => {}
        Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => return Ok(0),
        Err(e) => return Err(format!("Couldn’t read Credential Manager: {e}")),
    }
    let mut ours: Vec<Vec<u16>> = Vec::new();
    for &credential in unsafe { std::slice::from_raw_parts(list, count as usize) } {
        let credential = unsafe { &*credential };
        let target = unsafe { credential.TargetName.to_string() }.unwrap_or_default();
        let user = unsafe { credential.UserName.to_string() }.unwrap_or_default();
        if credential.Type == CRED_TYPE_GENERIC && is_saved_key(&target, &user) {
            ours.push(target.encode_utf16().chain(Some(0)).collect());
        }
    }
    unsafe { CredFree(list.cast()) };
    let mut deleted = 0;
    for target in ours {
        match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Ok(()) => deleted += 1,
            Err(e) => return Err(format!("Couldn’t delete a saved key: {e}")),
        }
    }
    Ok(deleted)
}

/// Keys saved in memory, for tests.
#[cfg(test)]
#[derive(Default)]
pub struct FakeSecrets {
    /// Saved keys in the current service by account name (see [`account`]).
    pub saved: std::sync::Mutex<std::collections::HashMap<String, String>>,
    pub old: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// Refuse every save, as a locked or missing credential store does.
    pub refuse: bool,
    pub refuse_read: std::sync::atomic::AtomicBool,
    pub unavailable: bool,
    pub refuse_delete: std::sync::Mutex<Option<String>>,
}

#[cfg(test)]
impl FakeSecrets {
    pub fn key(&self, kind: ProviderKind, profile_id: &str) -> Option<String> {
        self.saved.lock().unwrap().get(&account(kind, profile_id)).cloned()
    }
}

#[cfg(test)]
impl Secrets for FakeSecrets {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        if self.unavailable || self.refuse_read.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(format!("Couldn’t read the key from the {STORE}: it is locked"));
        }
        Ok(self.saved.lock().unwrap().get(account).cloned())
    }

    fn set(&self, kind: ProviderKind, profile_id: &str, key: &str) -> Result<(), String> {
        if self.refuse || self.unavailable {
            return Err(format!("Couldn’t save the key to the {STORE}: it is locked"));
        }
        self.saved.lock().unwrap().insert(account(kind, profile_id), key.to_string());
        Ok(())
    }

    fn delete_entry(&self, service: &str, account: &str) -> Result<bool, String> {
        if self.unavailable || self.refuse_delete.lock().unwrap().as_deref() == Some(account) {
            return Err(format!("Couldn’t delete a key from the {STORE}: it is locked"));
        }
        let keys = if service == SERVICE { &self.saved } else { &self.old };
        Ok(keys.lock().unwrap().remove(account).is_some())
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

    #[test]
    fn deleting_a_key_also_deletes_copies_from_earlier_versions() {
        assert_eq!(
            entries_to_delete(ProviderKind::Gemini, "gemini"),
            [
                (SERVICE, "gemini:gemini".to_string()),
                (SERVICE, "gemini".to_string()),
                (OLD_SERVICE, "gemini".to_string()),
            ]
        );
    }

    #[test]
    fn only_keys_saved_under_supereditrs_service_count() {
        assert!(is_saved_key("openai-1.app.supereditr", "openai-1"));
        assert!(is_saved_key("anthropic:anthropic.app.supereditr", "anthropic:anthropic"), "an account name");
        assert!(is_saved_key("a.b.app.supereditr", "a.b"), "a profile ID with a dot");
        assert!(!is_saved_key("openai-1.app.supereditr", "anthropic-2"), "a user name that doesn't match");
        assert!(is_saved_key("openai-1.com.sghael.supereditr", "openai-1"), "the service before 0.1.2");
        assert!(!is_saved_key("openai-1.com.example.notes", "openai-1"), "another service");
        assert!(!is_saved_key("openai-1.app.supereditr.backup", "openai-1"));
        assert!(!is_saved_key("openai-1app.supereditr", "openai-1"));
        assert!(!is_saved_key("app.supereditr", ""), "no profile ID at all");
    }
}
