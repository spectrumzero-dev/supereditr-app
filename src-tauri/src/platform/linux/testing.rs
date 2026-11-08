//! A fake desktop, accessibility bus and clipboard for the chains' tests. Keys sent to the fake
//! window act as an app would: the copy key copies its selection, and Ctrl+V pastes the clipboard
//! over it.

use super::atspi::{Field, FieldId, Fields, ReadError};
use super::capture::{self, Services};
use super::clipboard::fake::FakeClipboard;
use super::desktop::{Capabilities, Desktop, DesktopError, SetupCheck, Shortcut, Subscription, WindowRef};
use super::{Capture, replace};
use crate::platform::{PlatformError, Rect, ReplaceOutcome};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

pub fn window(app_id: &str) -> WindowRef {
    WindowRef {
        handle: "0x562229d30760".into(),
        pid: 4242,
        app_id: app_id.into(),
        title: "Notes".into(),
        rect: Some(Rect { x: 12.0, y: 38.0, width: 1256.0, height: 750.0 }),
        xwayland: false,
        tags: Vec::new(),
    }
}

/// An editable field whose selection, from its start, is `selected`.
pub fn field(selected: &str) -> Field {
    Field {
        id: FieldId { bus: ":1.42".into(), path: "/org/a11y/atspi/accessible/17".into() },
        editable: true,
        password: false,
        start: 0,
        end: selected.chars().count() as i32,
        selected: selected.into(),
        caret: selected.chars().count() as i32,
        bounds: None,
        window_bounds: None,
        code: Vec::new(),
        embedded: false,
    }
}

#[derive(Default)]
pub struct FakeFields {
    field: Mutex<Option<Field>>,
    sentence: Mutex<Option<Field>>,
    /// The field's whole text.
    content: Mutex<String>,
    /// The app doesn't answer, as when it's frozen.
    frozen: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl Fields for FakeFields {
    async fn focused(&self, _pid: i32) -> Result<Option<Field>, ReadError> {
        if self.frozen.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(ReadError::Timeout);
        }
        Ok(self.field.lock().unwrap().clone())
    }

    async fn text(&self, _id: &FieldId, start: i32, end: i32) -> Result<String, ReadError> {
        Ok(self.content.lock().unwrap().chars().skip(start as usize).take((end - start).max(0) as usize).collect())
    }

    async fn whole_text(&self, _field: &Field) -> Result<String, ReadError> {
        Ok(self.content.lock().unwrap().clone())
    }

    async fn select_sentence(&self, _field: &Field) -> Result<Option<Field>, ReadError> {
        let sentence = self.sentence.lock().unwrap().clone();
        if sentence.is_some() {
            *self.field.lock().unwrap() = sentence.clone();
        }
        Ok(sentence)
    }
}

pub struct FakeDesktop {
    target: WindowRef,
    front: Mutex<String>,
    fallback: bool,
    refuse_focus: bool,
    /// What the app copies when sent its copy key; nothing when nothing is selected.
    copies: Option<String>,
    /// Whether Ctrl+V pastes the clipboard into the field.
    pastes: bool,
    keys: Mutex<Vec<String>>,
    focus_requests: Mutex<usize>,
    clipboard: Arc<FakeClipboard>,
    fields: Arc<FakeFields>,
}

#[async_trait]
impl Desktop for FakeDesktop {
    fn name(&self) -> &'static str {
        "fake"
    }

    async fn capabilities(&self, _window: &WindowRef) -> Capabilities {
        if self.fallback { Capabilities::default() } else { Capabilities { send_keys: true, focus: true } }
    }

    fn front_window(&self) -> Result<WindowRef, DesktopError> {
        if self.fallback {
            return Err(DesktopError::Unsupported);
        }
        // Anything else in front is SuperEditr's panel.
        let front = self.front.lock().unwrap().clone();
        if front == self.target.handle {
            Ok(self.target.clone())
        } else {
            Ok(WindowRef { handle: front, pid: 1, app_id: "supereditr".into(), ..self.target.clone() })
        }
    }

    fn watch_front_window(&self, _on_change: Box<dyn Fn(WindowRef) + Send + Sync>) -> Subscription {
        Subscription::none()
    }

    fn cursor(&self) -> Result<(f64, f64), DesktopError> {
        Ok((640.0, 400.0))
    }

    fn place_overlay(&self, _at: Rect, _shown: bool) -> Result<(), DesktopError> {
        Ok(())
    }

    async fn focus(&self, window: &WindowRef) -> Result<(), DesktopError> {
        *self.focus_requests.lock().unwrap() += 1;
        if !self.refuse_focus {
            *self.front.lock().unwrap() = window.handle.clone();
        }
        Ok(())
    }

    async fn focus_overlay(&self) -> Result<(), DesktopError> {
        Ok(())
    }

    async fn send_shortcut(&self, _window: &WindowRef, keys: Shortcut) -> Result<(), DesktopError> {
        let mods = if keys.modifiers == crate::double_press::CONTROL { "CTRL" } else { "?" };
        self.keys.lock().unwrap().push(format!("{mods} {}", keys.key));
        match keys.key.as_str() {
            "C" | "Insert" => {
                if let Some(text) = &self.copies {
                    self.clipboard.copy_from_app(text);
                }
            }
            "V" if self.pastes => {
                let pasted = String::from_utf8(self.clipboard.get("text/plain;charset=utf-8").unwrap()).unwrap();
                let field = self.fields.field.lock().unwrap().clone().unwrap();
                let mut content = self.fields.content.lock().unwrap();
                let chars: Vec<char> = content.chars().collect();
                *content = chars[..field.start as usize]
                    .iter()
                    .chain(pasted.chars().collect::<Vec<_>>().iter())
                    .chain(chars[field.end as usize..].iter())
                    .collect();
            }
            _ => {}
        }
        Ok(())
    }

    fn setup_status(&self) -> Vec<SetupCheck> {
        Vec::new()
    }
}

/// An app's window, its focused field and the clipboard, with the user's own copy on it.
pub struct World {
    desktop: FakeDesktop,
    fields: Arc<FakeFields>,
    pub clipboard: Arc<FakeClipboard>,
}

impl World {
    pub fn new(app_id: &str) -> Self {
        let clipboard = Arc::new(FakeClipboard::holding(&[
            ("text/plain;charset=utf-8", b"Rhyan's own copy"),
            ("text/html", b"<b>Rhyan's</b> own copy"),
        ]));
        let fields = Arc::new(FakeFields::default());
        let target = window(app_id);
        let desktop = FakeDesktop {
            front: Mutex::new(target.handle.clone()),
            target,
            fallback: false,
            refuse_focus: false,
            copies: None,
            pastes: false,
            keys: Mutex::default(),
            focus_requests: Mutex::default(),
            clipboard: clipboard.clone(),
            fields: fields.clone(),
        };
        Self { desktop, fields, clipboard }
    }

    pub fn with_field(self, field: Field) -> Self {
        *self.fields.content.lock().unwrap() = format!("{} too the store tomorow.", field.selected);
        *self.fields.field.lock().unwrap() = Some(field);
        self
    }

    pub fn with_sentence(self, field: Field) -> Self {
        *self.fields.sentence.lock().unwrap() = Some(field);
        self
    }

    pub fn copying(mut self, text: &str) -> Self {
        self.desktop.copies = Some(text.into());
        self
    }

    pub fn pasting(mut self) -> Self {
        self.desktop.pastes = true;
        self
    }

    pub fn refusing_focus(mut self) -> Self {
        self.desktop.refuse_focus = true;
        self
    }

    /// The app stops answering AT-SPI, and copies nothing.
    pub fn frozen(self) -> Self {
        self.fields.frozen.store(true, std::sync::atomic::Ordering::Relaxed);
        self
    }

    pub fn on_fallback(mut self) -> Self {
        self.desktop.fallback = true;
        self
    }

    fn services(&self) -> Services<'_> {
        Services { desktop: &self.desktop, fields: self.fields.as_ref(), clipboard: self.clipboard.as_ref() }
    }

    pub fn capture(&self, allow_copy: bool, select_sentence: bool) -> Result<Capture, PlatformError> {
        tauri::async_runtime::block_on(capture::capture(&self.services(), allow_copy, select_sentence))
    }

    pub fn replace(&self, capture: &Capture, text: &str) -> ReplaceOutcome {
        tauri::async_runtime::block_on(replace::replace(&self.services(), capture, text))
    }

    pub fn keys_sent(&self) -> Vec<String> {
        self.desktop.keys.lock().unwrap().clone()
    }

    pub fn clear_keys(&self) {
        self.desktop.keys.lock().unwrap().clear();
        *self.clipboard.offers_made.lock().unwrap() = 0;
    }

    pub fn clipboard_offers(&self) -> usize {
        *self.clipboard.offers_made.lock().unwrap()
    }

    pub fn focus_requests(&self) -> usize {
        *self.desktop.focus_requests.lock().unwrap()
    }

    pub fn field_text(&self) -> String {
        self.fields.content.lock().unwrap().clone()
    }

    /// SuperEditr's panel takes focus, as during a review started with the shortcut.
    pub fn put_panel_in_front(&self) {
        *self.desktop.front.lock().unwrap() = "0xpanel".into();
    }

    /// The user selects something else in the field.
    pub fn select(&self, start: i32, end: i32) {
        let content = self.field_text();
        let mut fields = self.fields.field.lock().unwrap();
        let field = fields.as_mut().unwrap();
        field.start = start;
        field.end = end;
        field.selected = content.chars().skip(start as usize).take((end - start) as usize).collect();
    }
}
