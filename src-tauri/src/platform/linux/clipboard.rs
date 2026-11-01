//! The clipboard, through Wayland's data-control protocol, which reads and sets it without
//! SuperEditr having focus.
//!
//! Pasting a correction and the guarded copy each run as a [`Transaction`]: the user's clipboard is
//! put aside with every type it offers, SuperEditr's own content goes in, and the user's comes back
//! afterwards, unless something newer was copied meanwhile. Reads are bounded in size and time, so
//! an app that holds the clipboard but never answers can't hang SuperEditr.
//!
//! Everything SuperEditr puts on the clipboard, a restore included, carries the password-manager
//! hint, so clipboard history (Omarchy's included) leaves it out.

use std::time::{Duration, Instant};

pub const SENSITIVE_HINT: &str = "x-kde-passwordManagerHint";

/// Offered with SuperEditr's own content, with a different id per transaction, so a transaction
/// can tell whether the clipboard is still its own.
const MARKER: &str = "application/x-supereditr-transaction;id=";

/// The text types SuperEditr offers, as `wl-copy` does.
const TEXT_TYPES: &[&str] = &["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING", "TEXT"];

/// The types the text of a copy is read from, best first.
const READ_TEXT_TYPES: &[&str] = &["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING", "TEXT"];

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_types: usize,
    pub max_bytes_per_type: usize,
    pub max_bytes: usize,
    pub read_timeout: Duration,
    pub total_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_types: 32,
            max_bytes_per_type: 16 << 20,
            max_bytes: 32 << 20,
            read_timeout: Duration::from_millis(200),
            total_timeout: Duration::from_millis(600),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    #[error("the clipboard isn’t available: {0}")]
    Unavailable(String),
    #[error("the app holding the clipboard didn’t answer in time")]
    Timeout,
    #[error("the clipboard holds more than SuperEditr keeps aside")]
    TooLarge,
    #[error("{0}")]
    Failed(String),
}

/// The clipboard as data-control sees it. [`Wayland`] is the real one; tests use a fake.
pub trait DataControl: Send + Sync {
    /// The types on offer, in the order offered; empty when the clipboard is empty.
    fn offers(&self) -> Result<Vec<String>, ClipboardError>;
    /// One type's data, at most `limit` bytes.
    fn read(&self, mime: &str, limit: usize, timeout: Duration) -> Result<Vec<u8>, ClipboardError>;
    /// Replaces the clipboard with exactly these types.
    fn offer(&self, items: Vec<(String, Vec<u8>)>) -> Result<(), ClipboardError>;
    fn clear(&self) -> Result<(), ClipboardError>;
}

/// The user's clipboard, put aside.
#[derive(Debug, Default)]
pub struct Snapshot {
    items: Vec<(String, Vec<u8>)>,
    /// Types that couldn't be kept (too many, too large, or the app didn't answer): a restore
    /// leaves them out.
    pub skipped: Vec<String>,
}

impl Snapshot {
    pub fn take(clipboard: &dyn DataControl, limits: &Limits) -> Result<Self, ClipboardError> {
        let started = Instant::now();
        let mut snapshot = Snapshot::default();
        let mut used = 0;
        for mime in clipboard.offers()? {
            if mime.starts_with(MARKER) || snapshot.items.iter().any(|(m, _)| *m == mime) {
                continue;
            }
            let left = limits.total_timeout.saturating_sub(started.elapsed());
            let room = limits.max_bytes_per_type.min(limits.max_bytes - used);
            if snapshot.items.len() >= limits.max_types || left.is_zero() || room == 0 {
                snapshot.skipped.push(mime);
                continue;
            }
            match clipboard.read(&mime, room, left.min(limits.read_timeout)) {
                Ok(data) => {
                    used += data.len();
                    snapshot.items.push((mime, data));
                }
                Err(_) => snapshot.skipped.push(mime),
            }
        }
        Ok(snapshot)
    }

    pub fn types(&self) -> impl Iterator<Item = &str> {
        self.items.iter().map(|(mime, _)| mime.as_str())
    }
}

/// What a transaction's restore did.
#[derive(Debug, PartialEq, Eq)]
pub enum Restore {
    /// The user's clipboard is back, every type it had (less [`Snapshot::skipped`]).
    Restored,
    /// Something newer was copied after SuperEditr's content, so the clipboard was left alone.
    Superseded,
    Failed(String),
}

/// SuperEditr's use of the clipboard, from putting the user's aside to giving it back.
pub struct Transaction<'a> {
    clipboard: &'a dyn DataControl,
    before: Snapshot,
    marker: String,
}

impl<'a> Transaction<'a> {
    pub fn begin(clipboard: &'a dyn DataControl, limits: &Limits) -> Result<Self, ClipboardError> {
        let before = Snapshot::take(clipboard, limits)?;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Self { clipboard, before, marker: format!("{MARKER}{}-{id}", std::process::id()) })
    }

    pub fn before(&self) -> &Snapshot {
        &self.before
    }

    /// Puts `text` on the clipboard, marked sensitive and as this transaction's.
    pub fn put_text(&self, text: &str) -> Result<(), ClipboardError> {
        let mut items = text_items(text);
        items.push((self.marker.clone(), Vec::new()));
        self.clipboard.offer(items)
    }

    /// Puts only this transaction's marker on the clipboard, so a copy that follows shows as the
    /// marker going away.
    pub fn put_marker(&self) -> Result<(), ClipboardError> {
        self.clipboard.offer(vec![(SENSITIVE_HINT.into(), b"secret".to_vec()), (self.marker.clone(), Vec::new())])
    }

    /// Whether the clipboard still holds what this transaction put there.
    pub fn owned(&self) -> bool {
        self.clipboard.offers().is_ok_and(|offers| offers.contains(&self.marker))
    }

    /// Waits for something else to replace this transaction's content, as a copy in the target app
    /// does. False when nothing did within `timeout`.
    pub fn wait_for_copy(&self, timeout: Duration) -> bool {
        let started = Instant::now();
        loop {
            if !self.owned() {
                return true;
            }
            if started.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The clipboard's text now.
    pub fn read_text(&self, limits: &Limits) -> Result<String, ClipboardError> {
        let offers = self.clipboard.offers()?;
        let mime = READ_TEXT_TYPES
            .iter()
            .find(|t| offers.iter().any(|o| o == *t))
            .ok_or_else(|| ClipboardError::Failed("the copy has no text".into()))?;
        let data = self.clipboard.read(mime, limits.max_bytes_per_type, limits.read_timeout)?;
        String::from_utf8(data).map_err(|_| ClipboardError::Failed("the copied text isn’t UTF-8".into()))
    }

    /// Gives the user's clipboard back, unless something newer than this transaction's content was
    /// copied.
    pub fn restore(self) -> Restore {
        if !self.owned() {
            return Restore::Superseded;
        }
        self.put_back()
    }

    /// Gives the user's clipboard back whatever it holds now: after a guarded copy, the target
    /// app's copy has replaced SuperEditr's marker, and it goes too.
    pub fn restore_after_copy(self) -> Restore {
        self.put_back()
    }

    fn put_back(self) -> Restore {
        let result = if self.before.items.is_empty() {
            self.clipboard.clear()
        } else {
            let mut items = self.before.items;
            if !items.iter().any(|(mime, _)| mime == SENSITIVE_HINT) {
                items.push((SENSITIVE_HINT.into(), b"secret".to_vec()));
            }
            self.clipboard.offer(items)
        };
        match result {
            Ok(()) => Restore::Restored,
            Err(error) => Restore::Failed(error.to_string()),
        }
    }
}

/// `text` as every text type, with the sensitive hint.
fn text_items(text: &str) -> Vec<(String, Vec<u8>)> {
    let mut items: Vec<(String, Vec<u8>)> =
        TEXT_TYPES.iter().map(|mime| (mime.to_string(), text.as_bytes().to_vec())).collect();
    items.push((SENSITIVE_HINT.into(), b"secret".to_vec()));
    items
}

/// Puts `text` on the clipboard for the user to paste, marked sensitive.
pub fn copy_text(clipboard: &dyn DataControl, text: &str) -> Result<(), ClipboardError> {
    clipboard.offer(text_items(text))
}

/// The session's clipboard, through wl-clipboard-rs.
pub struct Wayland;

impl DataControl for Wayland {
    fn offers(&self) -> Result<Vec<String>, ClipboardError> {
        use wl_clipboard_rs::paste::{ClipboardType, Error, Seat, get_mime_types_ordered};
        match get_mime_types_ordered(ClipboardType::Regular, Seat::Unspecified) {
            Ok(types) => Ok(types),
            Err(Error::ClipboardEmpty | Error::NoMimeType) => Ok(Vec::new()),
            Err(error) => Err(ClipboardError::Unavailable(error.to_string())),
        }
    }

    fn read(&self, mime: &str, limit: usize, timeout: Duration) -> Result<Vec<u8>, ClipboardError> {
        use std::io::Read;
        use wl_clipboard_rs::paste::{ClipboardType, MimeType, Seat, get_contents};
        let (send, receive) = std::sync::mpsc::channel();
        let mime = mime.to_string();
        // The read blocks until the owner writes. On a timeout the thread is left to finish (or
        // not) on its own, and its result is dropped.
        std::thread::spawn(move || {
            let result = get_contents(ClipboardType::Regular, Seat::Unspecified, MimeType::Specific(&mime))
                .map_err(|error| ClipboardError::Failed(error.to_string()))
                .and_then(|(pipe, _)| {
                    let mut data = Vec::new();
                    pipe.take(limit as u64 + 1)
                        .read_to_end(&mut data)
                        .map_err(|error| ClipboardError::Failed(error.to_string()))?;
                    if data.len() > limit { Err(ClipboardError::TooLarge) } else { Ok(data) }
                });
            let _ = send.send(result);
        });
        receive.recv_timeout(timeout).map_err(|_| ClipboardError::Timeout)?
    }

    fn offer(&self, items: Vec<(String, Vec<u8>)>) -> Result<(), ClipboardError> {
        use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};
        let sources = items
            .into_iter()
            .map(|(mime, data)| MimeSource { source: Source::Bytes(data.into()), mime_type: MimeType::Specific(mime) })
            .collect();
        let mut options = Options::new();
        options.omit_additional_text_mime_types(true);
        options.copy_multi(sources).map_err(|error| ClipboardError::Failed(error.to_string()))
    }

    fn clear(&self) -> Result<(), ClipboardError> {
        use wl_clipboard_rs::copy::{ClipboardType, Seat, clear};
        clear(ClipboardType::Regular, Seat::All).map_err(|error| ClipboardError::Failed(error.to_string()))
    }
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::Mutex;

    /// A clipboard in memory. `slow` types never answer; `on_offer` runs after each offer, as an
    /// app copying in response to a key would.
    #[derive(Default)]
    pub struct FakeClipboard {
        pub items: Mutex<Vec<(String, Vec<u8>)>>,
        pub slow: Vec<String>,
        pub offers_made: Mutex<usize>,
    }

    impl FakeClipboard {
        pub fn holding(items: &[(&str, &[u8])]) -> Self {
            Self {
                items: Mutex::new(items.iter().map(|(m, d)| (m.to_string(), d.to_vec())).collect()),
                ..Self::default()
            }
        }

        pub fn get(&self, mime: &str) -> Option<Vec<u8>> {
            self.items.lock().unwrap().iter().find(|(m, _)| m == mime).map(|(_, d)| d.clone())
        }

        pub fn types(&self) -> Vec<String> {
            self.items.lock().unwrap().iter().map(|(m, _)| m.clone()).collect()
        }

        /// Another app copies.
        pub fn copy_from_app(&self, text: &str) {
            *self.items.lock().unwrap() = vec![
                ("text/plain;charset=utf-8".into(), text.as_bytes().to_vec()),
                ("text/html".into(), b"<b>x</b>".to_vec()),
            ];
        }
    }

    impl DataControl for FakeClipboard {
        fn offers(&self) -> Result<Vec<String>, ClipboardError> {
            Ok(self.types())
        }

        fn read(&self, mime: &str, limit: usize, _timeout: Duration) -> Result<Vec<u8>, ClipboardError> {
            if self.slow.iter().any(|m| m == mime) {
                return Err(ClipboardError::Timeout);
            }
            let data = self.get(mime).ok_or_else(|| ClipboardError::Failed("no such type".into()))?;
            if data.len() > limit { Err(ClipboardError::TooLarge) } else { Ok(data) }
        }

        fn offer(&self, items: Vec<(String, Vec<u8>)>) -> Result<(), ClipboardError> {
            *self.items.lock().unwrap() = items;
            *self.offers_made.lock().unwrap() += 1;
            Ok(())
        }

        fn clear(&self) -> Result<(), ClipboardError> {
            self.items.lock().unwrap().clear();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeClipboard;
    use super::*;

    /// A copy from Chromium: plain text, HTML and Chromium's own types.
    fn chromium_copy() -> FakeClipboard {
        FakeClipboard::holding(&[
            ("text/plain;charset=utf-8", b"Brynlee's KPI notes"),
            ("text/html", b"<meta charset='utf-8'><b>Brynlee's</b> KPI notes"),
            ("chromium/x-source-url", b"https://quillpoint.example/okr"),
            ("chromium/x-internal-source-rfh-token", b"\x01\x02\x03"),
        ])
    }

    #[test]
    fn a_paste_puts_the_text_aside_and_restores_every_type() {
        let clipboard = chromium_copy();
        let before = clipboard.types();
        let transaction = Transaction::begin(&clipboard, &Limits::default()).unwrap();
        transaction.put_text("They’re going").unwrap();
        assert_eq!(clipboard.get("text/plain").unwrap(), "They’re going".as_bytes());
        assert_eq!(clipboard.get(SENSITIVE_HINT).unwrap(), b"secret", "history skips the correction");
        assert!(transaction.owned());
        assert_eq!(transaction.restore(), Restore::Restored);
        for mime in &before {
            assert_eq!(clipboard.get(mime), chromium_copy().get(mime), "{mime} comes back as it was");
        }
        assert_eq!(clipboard.get(SENSITIVE_HINT).unwrap(), b"secret", "history skips the restore too");
        assert!(!clipboard.types().iter().any(|m| m.starts_with(MARKER)));
    }

    #[test]
    fn a_newer_copy_is_never_overwritten_by_the_restore() {
        let clipboard = chromium_copy();
        let transaction = Transaction::begin(&clipboard, &Limits::default()).unwrap();
        transaction.put_text("They’re going").unwrap();
        clipboard.copy_from_app("Kyndal's newer copy");
        assert!(!transaction.owned());
        assert_eq!(transaction.restore(), Restore::Superseded);
        assert_eq!(clipboard.get("text/plain;charset=utf-8").unwrap(), b"Kyndal's newer copy");
    }

    #[test]
    fn an_empty_clipboard_is_empty_again_afterwards() {
        let clipboard = FakeClipboard::default();
        let transaction = Transaction::begin(&clipboard, &Limits::default()).unwrap();
        assert_eq!(transaction.before().types().count(), 0);
        transaction.put_text("They’re going").unwrap();
        assert_eq!(transaction.restore(), Restore::Restored);
        assert!(clipboard.types().is_empty());
    }

    #[test]
    fn a_type_that_doesnt_answer_or_is_too_big_is_skipped_not_waited_for() {
        let mut clipboard = chromium_copy();
        clipboard.slow = vec!["text/html".into()];
        let limits = Limits { max_bytes_per_type: 20, ..Limits::default() };
        let snapshot = Snapshot::take(&clipboard, &limits).unwrap();
        assert_eq!(snapshot.skipped, ["text/html", "chromium/x-source-url"]);
        assert_eq!(
            snapshot.types().collect::<Vec<_>>(),
            ["text/plain;charset=utf-8", "chromium/x-internal-source-rfh-token"]
        );

        let limits = Limits { max_types: 1, ..Limits::default() };
        assert_eq!(Snapshot::take(&chromium_copy(), &limits).unwrap().skipped.len(), 3);
    }

    #[test]
    fn a_guarded_copy_sees_the_apps_copy_and_then_restores() {
        let clipboard = chromium_copy();
        let limits = Limits::default();
        let transaction = Transaction::begin(&clipboard, &limits).unwrap();
        transaction.put_marker().unwrap();
        assert!(!transaction.wait_for_copy(Duration::from_millis(20)), "nothing copied yet");
        clipboard.copy_from_app("Their going too the store");
        assert!(transaction.wait_for_copy(Duration::from_millis(20)));
        assert_eq!(transaction.read_text(&limits).unwrap(), "Their going too the store");
        assert_eq!(transaction.restore_after_copy(), Restore::Restored);
        assert_eq!(clipboard.get("chromium/x-source-url").unwrap(), b"https://quillpoint.example/okr");
    }

    #[test]
    fn text_put_for_pasting_by_hand_is_marked_sensitive() {
        let clipboard = FakeClipboard::default();
        copy_text(&clipboard, "Rhyan").unwrap();
        assert_eq!(clipboard.get("UTF8_STRING").unwrap(), b"Rhyan");
        assert!(clipboard.types().contains(&SENSITIVE_HINT.to_string()));
    }
}
