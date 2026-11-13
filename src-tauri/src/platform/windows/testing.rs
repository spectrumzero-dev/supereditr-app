//! A pretend desktop for testing the capture and replace chains: one app window holding a document
//! with a selection, SuperEditr's panel, UI Automation over the document, and a clipboard.

use super::Capture;
use super::capture::{self, Desktop, InputWatch, Key, Services};
use super::clipboard::fake::Fake;
use super::clipboard::{CF_UNICODETEXT, Clipboard, Raw, utf16z};
use super::replace;
use super::uia::{CONTEXT, Field, Fields, PxRect, ReadError, Recheck};
use super::window::WindowRef;
use crate::platform::{PlatformError, ReplaceOutcome};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const HTML: u32 = 0xC100;

/// A field holding `text`, all of it selected.
pub fn field(text: &str) -> Field {
    let units: Vec<u16> = text.encode_utf16().collect();
    Field {
        token: 0,
        pid: 42,
        control_type: 50004,
        runtime_id: vec![42, 0x1_0000, 7],
        // The test app's window.
        root_window: Some(100),
        class: String::new(),
        framework: "Win32".into(),
        name: String::new(),
        password: false,
        read_only: Some(false),
        composition: false,
        ranges: 1,
        start: 0,
        end: units.len(),
        text: units,
        before: Vec::new(),
        after: Vec::new(),
        bounds: Some(PxRect { left: 200, top: 300, right: 260, bottom: 320 }),
        element_bounds: None,
    }
}

type Stored = (u64, u64, (usize, usize));

#[derive(Default)]
struct Document {
    text: Vec<u16>,
    selection: (usize, usize),
    /// Which element has focus; a new one means focus moved.
    element: u64,
}

pub struct World {
    image: String,
    app: WindowRef,
    panel: WindowRef,
    foreground: Arc<Mutex<Option<WindowRef>>>,
    doc: Arc<Mutex<Document>>,
    /// The field's other properties, or None when UI Automation finds no field.
    template: Mutex<Option<Field>>,
    /// The latest read: its token, the focused element, and the selection.
    stored: Mutex<Option<Stored>>,
    tokens: AtomicU64,
    reads: AtomicUsize,
    late: AtomicUsize,
    keys: Mutex<Vec<String>>,
    copying: Mutex<Option<String>>,
    /// The app takes this long to act on each copy key.
    copy_delay: Mutex<Option<Duration>>,
    paste_lands: AtomicBool,
    refusing_focus: AtomicBool,
    elevated: AtomicBool,
    caret: AtomicBool,
    focus_requests: AtomicUsize,
    on_focus: Mutex<Option<(usize, usize)>>,
    /// The time each read of the focused field was given.
    read_limits: Mutex<Vec<Duration>>,
    /// How many more rechecks answer without the field's text.
    rechecks_without_text: AtomicUsize,
    /// Runs while SuperEditr waits for the user to let go of their modifiers.
    on_release: Mutex<Option<Hook>>,
    /// Runs while the focused field is read.
    on_read: Mutex<Option<Hook>>,
    /// Counts the user's other keyboard and mouse input, which SuperEditr ignores.
    inputs: Arc<AtomicUsize>,
    /// Counts the copy keys the user pressed themselves.
    copy_keys: Arc<AtomicUsize>,
    /// The user presses Ctrl+C themselves while SuperEditr waits for the app to copy.
    copy_during_wait: AtomicBool,
    pub clipboard: Clipboard<Fake>,
}

type Hook = Box<dyn FnOnce(&World) + Send>;

/// Another app's window.
const OTHER_WINDOW: WindowRef = WindowRef { hwnd: 300, pid: 77 };

impl World {
    pub fn new(image: &str) -> Self {
        let app = WindowRef { hwnd: 100, pid: 42 };
        World {
            image: image.into(),
            app,
            panel: WindowRef { hwnd: 900, pid: std::process::id() },
            foreground: Arc::new(Mutex::new(Some(app))),
            doc: Arc::default(),
            template: Mutex::new(None),
            stored: Mutex::new(None),
            tokens: AtomicU64::new(0),
            reads: AtomicUsize::new(0),
            late: AtomicUsize::new(0),
            keys: Mutex::default(),
            copying: Mutex::new(None),
            copy_delay: Mutex::new(None),
            paste_lands: AtomicBool::new(true),
            refusing_focus: AtomicBool::new(false),
            elevated: AtomicBool::new(false),
            caret: AtomicBool::new(true),
            focus_requests: AtomicUsize::new(0),
            on_focus: Mutex::new(None),
            on_release: Mutex::new(None),
            on_read: Mutex::new(None),
            inputs: Arc::default(),
            copy_keys: Arc::default(),
            copy_during_wait: AtomicBool::new(false),
            rechecks_without_text: AtomicUsize::new(0),
            read_limits: Mutex::default(),
            clipboard: Clipboard::new(Fake::with(&[
                (CF_UNICODETEXT, &utf16z("Rhyan's own copy")),
                (HTML, b"<b>Rhyan's own copy</b>"),
            ])),
        }
    }

    /// A field whose whole text is selected, with `field`'s properties.
    pub fn with_field(self, field: Field) -> Self {
        {
            let mut doc = self.doc.lock().unwrap();
            doc.text = field.text.clone();
            doc.selection = (0, doc.text.len());
        }
        *self.template.lock().unwrap() = Some(field);
        self
    }

    /// An editable field holding `text`, with the first `selected` in it selected.
    pub fn with_document(self, text: &str, selected: &str) -> Self {
        let start = text.find(selected).map_or(0, |at| text[..at].encode_utf16().count());
        let world = self.with_field(field(text));
        world.doc.lock().unwrap().selection = (start, start + selected.encode_utf16().count());
        world
    }

    pub fn select(self, start: usize, end: usize) -> Self {
        self.select_now(start, end);
        self
    }

    pub fn select_now(&self, start: usize, end: usize) {
        self.doc.lock().unwrap().selection = (start, end);
    }

    /// The app answers its first `n` reads with no field, as Chromium does.
    pub fn answering_late(self, n: usize) -> Self {
        self.late.store(n, Ordering::SeqCst);
        self
    }

    /// The app copies `text` when sent the copy key.
    pub fn copying(self, text: &str) -> Self {
        *self.copying.lock().unwrap() = Some(text.into());
        self
    }

    /// The app copies `text` when sent the copy key, but only `after` that, as a busy app does.
    pub fn copying_late(self, text: &str, after: Duration) -> Self {
        *self.copy_delay.lock().unwrap() = Some(after);
        self.copying(text)
    }

    /// Ctrl+V reaches the app, which pastes too late to verify.
    pub fn pasting_late(self) -> Self {
        self.paste_lands.store(false, Ordering::SeqCst);
        self
    }

    pub fn refusing_focus(self) -> Self {
        self.refusing_focus.store(true, Ordering::SeqCst);
        self
    }

    pub fn elevated(self) -> Self {
        self.elevated.store(true, Ordering::SeqCst);
        self
    }

    pub fn no_caret(&self) {
        self.caret.store(false, Ordering::SeqCst);
    }

    pub fn window(&self) -> WindowRef {
        self.app
    }

    pub fn put_panel_in_front(&self) {
        *self.foreground.lock().unwrap() = Some(self.panel);
    }

    /// Coming back to the window selects `start..end`.
    pub fn on_focus_select(&self, start: usize, end: usize) {
        *self.on_focus.lock().unwrap() = Some((start, end));
    }

    /// The user changes the selection while the clipboard is being put aside.
    pub fn during_snapshot_select(&self, start: usize, end: usize) {
        let doc = self.doc.clone();
        *self.clipboard.raw().during_read.lock().unwrap() =
            Some(Box::new(move |_| doc.lock().unwrap().selection = (start, end)));
    }

    /// The user does something while SuperEditr waits for them to let go of the apply key's
    /// modifiers, after the fix went on the clipboard and just before the paste.
    pub fn while_modifiers_held(&self, act: impl FnOnce(&World) + Send + 'static) {
        *self.on_release.lock().unwrap() = Some(Box::new(act));
    }

    /// The app runs in process `pid`, which SuperEditr hasn't read in yet.
    pub fn in_process(mut self, pid: u32) -> Self {
        self.app.pid = pid;
        *self.foreground.lock().unwrap() = Some(self.app);
        self
    }

    pub fn read_limits(&self) -> Vec<Duration> {
        self.read_limits.lock().unwrap().clone()
    }

    /// The next `n` rechecks of the selection find no text, as Chromium answers right after its
    /// window comes back to the front.
    pub fn rechecks_without_text(&self, n: usize) {
        self.rechecks_without_text.store(n, Ordering::SeqCst);
    }

    /// Another window comes to the front: the user switched, or an app took the focus.
    pub fn other_window_in_front(&self) {
        *self.foreground.lock().unwrap() = Some(OTHER_WINDOW);
    }

    /// Another window comes to the front while the focused field is being read.
    pub fn other_window_in_front_during_read(&self) {
        let foreground = self.foreground.clone();
        *self.on_read.lock().unwrap() = Some(Box::new(move |_| *foreground.lock().unwrap() = Some(OTHER_WINDOW)));
    }

    /// The user types, presses Esc or uses the mouse: no copy key.
    pub fn user_input(&self) {
        self.inputs.fetch_add(1, Ordering::SeqCst);
    }

    /// The user presses Ctrl+C themselves right after SuperEditr's copy key, while it waits.
    pub fn user_presses_copy_during_the_wait(&self) {
        self.copy_during_wait.store(true, Ordering::SeqCst);
    }

    /// Another window comes to the front while the clipboard is being put aside.
    pub fn other_window_in_front_during_snapshot(&self) {
        let foreground = self.foreground.clone();
        *self.clipboard.raw().during_read.lock().unwrap() =
            Some(Box::new(move |_| *foreground.lock().unwrap() = Some(OTHER_WINDOW)));
    }

    /// A Ctrl+V the app received earlier and hadn't acted on reaches the field now, pasting what's on
    /// the clipboard at this moment.
    pub fn paste_now(&self) {
        if let Some(text) = self.clipboard.raw().text() {
            let mut doc = self.doc.lock().unwrap();
            let (start, end) = doc.selection;
            let pasted: Vec<u16> = text.replace("\r\n", "\n").encode_utf16().collect();
            let len = pasted.len();
            doc.text.splice(start..end, pasted);
            doc.selection = (start + len, start + len);
        }
    }

    pub fn user_copies_during_snapshot(&self, text: &str) {
        let text = text.to_string();
        *self.clipboard.raw().after_close.lock().unwrap() = Some(Box::new(move |fake| fake.copy_from_app(&text)));
    }

    pub fn clipboard_too_big(&self) {
        self.clipboard.raw().too_large.store(true, Ordering::SeqCst);
    }

    fn services(&self) -> Services<'_, Fake> {
        Services { desktop: self, fields: self, clipboard: &self.clipboard }
    }

    pub fn capture(&self, allow_copy: bool) -> Result<Capture, PlatformError> {
        capture::capture(&self.services(), allow_copy)
    }

    pub fn replace(&self, capture: &Capture, text: &str) -> ReplaceOutcome {
        self.clear_keys();
        replace::replace(&self.services(), capture, text, replace::Clip::Restore)
    }

    /// The user chose to replace without their clipboard put aside.
    pub fn replace_anyway(&self, capture: &Capture, text: &str) -> ReplaceOutcome {
        self.clear_keys();
        replace::replace(&self.services(), capture, text, replace::Clip::Leave)
    }

    /// The app that put the clipboard's text there takes `wait` to render it.
    pub fn clipboard_owner_stalls(&self, wait: Duration) {
        *self.clipboard.raw().stall.lock().unwrap() = Some((CF_UNICODETEXT, wait));
    }

    /// Whether another app could open the clipboard now.
    pub fn clipboard_is_free(&self) -> bool {
        let raw = self.clipboard.raw();
        let open = raw.open();
        if open {
            raw.close();
        }
        open
    }

    /// How many times a format was read from the clipboard.
    pub fn clipboard_reads(&self) -> usize {
        self.clipboard.raw().reads.load(Ordering::SeqCst)
    }

    pub fn keys_sent(&self) -> Vec<String> {
        self.keys.lock().unwrap().clone()
    }

    pub fn clear_keys(&self) {
        self.keys.lock().unwrap().clear();
    }

    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    pub fn focus_requests(&self) -> usize {
        self.focus_requests.load(Ordering::SeqCst)
    }

    pub fn document(&self) -> String {
        String::from_utf16_lossy(&self.doc.lock().unwrap().text)
    }

    pub fn clipboard_text(&self) -> Option<String> {
        self.clipboard.raw().text()
    }

    pub fn clipboard_has_html(&self) -> bool {
        self.clipboard.raw().get(HTML).is_some()
    }

    /// The clipboard's HTML Format bytes.
    pub fn clipboard_html(&self) -> Option<Vec<u8>> {
        self.clipboard.raw().get(HTML)
    }

    /// How many times SuperEditr emptied the clipboard to write to it.
    pub fn clipboard_writes(&self) -> usize {
        self.clipboard.raw().empties.load(Ordering::SeqCst)
    }

    fn app_in_front(&self) -> bool {
        *self.foreground.lock().unwrap() == Some(self.app)
    }
}

fn context(text: &[u16], start: usize, end: usize) -> (Vec<u16>, Vec<u16>) {
    (text[start.saturating_sub(CONTEXT)..start].to_vec(), text[end..(end + CONTEXT).min(text.len())].to_vec())
}

impl Fields for World {
    fn focused(&self, limit: Duration) -> Result<Field, ReadError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let act = self.on_read.lock().unwrap().take();
        if let Some(act) = act {
            act(self);
        }
        self.read_limits.lock().unwrap().push(limit);
        if self.late.load(Ordering::SeqCst) > 0 {
            self.late.fetch_sub(1, Ordering::SeqCst);
            return Err(ReadError::NoText);
        }
        let Some(mut field) = self.template.lock().unwrap().clone() else { return Err(ReadError::NoText) };
        if field.password {
            return Err(ReadError::Password);
        }
        let doc = self.doc.lock().unwrap();
        let (start, end) = doc.selection;
        field.token = self.tokens.fetch_add(1, Ordering::SeqCst) + 1;
        field.text = doc.text[start..end].to_vec();
        (field.start, field.end) = (start, end);
        (field.before, field.after) = context(&doc.text, start, end);
        *self.stored.lock().unwrap() = Some((field.token, doc.element, (start, end)));
        Ok(field)
    }

    fn recheck(&self, token: u64, _limit: Duration) -> Result<Recheck, ReadError> {
        if self.rechecks_without_text.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok() {
            return Err(ReadError::NoText);
        }
        let stored = self.stored.lock().unwrap().filter(|s| s.0 == token).ok_or(ReadError::Gone)?;
        let doc = self.doc.lock().unwrap();
        let (start, end) = doc.selection;
        let (before, after) = context(&doc.text, start, end);
        Ok(Recheck {
            same_element: self.app_in_front() && doc.element == stored.1,
            ranges: 1,
            same_endpoints: (start, end) == stored.2,
            text: doc.text[start..end].to_vec(),
            before,
            after,
        })
    }

    fn document(&self, token: u64, _limit: Duration) -> Result<Vec<u16>, ReadError> {
        let stored = self.stored.lock().unwrap().filter(|s| s.0 == token).ok_or(ReadError::Gone)?;
        let doc = self.doc.lock().unwrap();
        if doc.element != stored.1 {
            return Err(ReadError::Gone);
        }
        Ok(doc.text.clone())
    }

    fn focused_is(&self, token: u64, _limit: Duration) -> bool {
        let stored = *self.stored.lock().unwrap();
        self.app_in_front() && stored.is_some_and(|s| s.0 == token && s.1 == self.doc.lock().unwrap().element)
    }
}

impl Desktop for World {
    fn foreground(&self) -> Option<WindowRef> {
        *self.foreground.lock().unwrap()
    }
    fn image_name(&self, pid: u32) -> Option<String> {
        (pid == self.app.pid).then(|| self.image.clone())
    }
    fn window_class(&self, _window: WindowRef) -> String {
        String::new()
    }
    fn elevated(&self, _pid: u32) -> bool {
        self.elevated.load(Ordering::SeqCst)
    }
    fn alive(&self, window: WindowRef) -> bool {
        window == self.app
    }
    fn bring_to_front(&self, window: WindowRef) -> bool {
        self.focus_requests.fetch_add(1, Ordering::SeqCst);
        if self.refusing_focus.load(Ordering::SeqCst) {
            return false;
        }
        *self.foreground.lock().unwrap() = Some(window);
        if let Some(selection) = self.on_focus.lock().unwrap().take() {
            self.doc.lock().unwrap().selection = selection;
        }
        true
    }
    fn release_modifiers(&self) {
        let act = self.on_release.lock().unwrap().take();
        if let Some(act) = act {
            act(self);
        }
    }
    fn send_ctrl(&self, key: Key) -> bool {
        self.keys.lock().unwrap().push(format!("Ctrl+{key:?}"));
        if self.copy_during_wait.swap(false, Ordering::SeqCst) {
            self.copy_keys.fetch_add(1, Ordering::SeqCst);
        }
        match key {
            Key::C | Key::Insert => {
                if let Some(text) = self.copying.lock().unwrap().clone() {
                    let app = super::apps::display_name(&self.image);
                    match *self.copy_delay.lock().unwrap() {
                        None => self.clipboard.raw().copy_from(&app, &text),
                        Some(after) => {
                            let raw = self.clipboard.raw_arc();
                            std::thread::spawn(move || {
                                std::thread::sleep(after);
                                raw.copy_from(&app, &text);
                            });
                        }
                    }
                }
            }
            Key::V => {
                if self.paste_lands.load(Ordering::SeqCst)
                    && let Some(text) = self.clipboard.raw().text()
                {
                    let mut doc = self.doc.lock().unwrap();
                    let (start, end) = doc.selection;
                    let pasted: Vec<u16> = text.replace("\r\n", "\n").encode_utf16().collect();
                    let len = pasted.len();
                    doc.text.splice(start..end, pasted);
                    doc.selection = (start + len, start + len);
                }
            }
        }
        true
    }
    fn caret(&self, _window: WindowRef) -> Option<PxRect> {
        self.caret.load(Ordering::SeqCst).then_some(PxRect { left: 300, top: 400, right: 302, bottom: 420 })
    }
    fn cursor(&self) -> Option<(i32, i32)> {
        Some((900, 500))
    }
    fn watch_copy_keys(&self) -> InputWatch {
        let copies = self.copy_keys.clone();
        let since = copies.load(Ordering::SeqCst);
        Box::new(move || copies.load(Ordering::SeqCst) != since)
    }
}
