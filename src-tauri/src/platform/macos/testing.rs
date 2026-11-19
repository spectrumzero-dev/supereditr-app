//! A pretend Mac for testing the replace chain: one app with one text field and a selection in it,
//! the general pasteboard, key events to the app, and a clock that moves only while SuperEditr
//! waits, so a test of a one-second wait takes no time.

use super::Capture;
use super::replace::{Clock, Desktop, Field, Pasteboard, Services, replace_with};
use super::text::Formatting;
use crate::platform::{PlatformError, ReplaceMethod};
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

/// The test app's process.
const PID: i32 = 42;

/// What the app does with an Accessibility write to its selected text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Write {
    Applies,
    /// Reports success and changes nothing, as some web views do.
    Ignored,
    /// Changes the field, but not to what was written: the last character goes missing.
    Garbled,
}

/// What the app does with a paste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pastes {
    Yes,
    /// The paste reaches the app but never shows up in the field.
    Never,
    /// The first `n` pastes are dropped, as Chromium drops one that races its clipboard notice.
    AfterDropping(usize),
}

type Hook = Box<dyn FnOnce(&World)>;

/// The clipboard's plain text and HTML.
type Clip = (String, Option<String>);

pub struct World {
    bundle_id: String,
    /// The field's whole text, as it really is.
    text: RefCell<String>,
    /// The selection: where it starts and how long it is, in UTF-16 units.
    selection: Cell<(usize, usize)>,
    focused: Cell<bool>,
    trusted: Cell<bool>,
    chromium: Cell<bool>,
    web: Cell<bool>,
    formatted: Cell<bool>,
    settable: Cell<bool>,
    write: Cell<Write>,
    moves_selection: Cell<bool>,
    pastes: Cell<Pastes>,
    /// Like Chromium's rich fields, `AXValue` leaves out empty paragraphs and the rebuilt web text
    /// has them.
    drops_empty_paragraphs: Cell<bool>,
    clipboard: RefCell<Clip>,
    change_count: Cell<isize>,
    after_paste: RefCell<Option<Hook>>,
    after_write: RefCell<Option<Hook>>,
    keys: RefCell<Vec<String>>,
    writes: RefCell<Vec<String>>,
    put_on_clipboard: RefCell<Vec<Clip>>,
    started: Instant,
    elapsed: Cell<Duration>,
}

impl World {
    /// The app `bundle_id`, with a native field whose selected text can be written, in front and
    /// focused, and the user's own copy on the clipboard.
    pub fn new(bundle_id: &str) -> Self {
        World {
            bundle_id: bundle_id.into(),
            text: RefCell::default(),
            selection: Cell::new((0, 0)),
            focused: Cell::new(true),
            trusted: Cell::new(true),
            chromium: Cell::new(false),
            web: Cell::new(false),
            formatted: Cell::new(false),
            settable: Cell::new(true),
            write: Cell::new(Write::Applies),
            moves_selection: Cell::new(true),
            pastes: Cell::new(Pastes::Yes),
            drops_empty_paragraphs: Cell::new(false),
            clipboard: RefCell::new(("Rhyan's own copy".into(), Some("<b>Rhyan's own copy</b>".into()))),
            change_count: Cell::new(1),
            after_paste: RefCell::default(),
            after_write: RefCell::default(),
            keys: RefCell::default(),
            writes: RefCell::default(),
            put_on_clipboard: RefCell::default(),
            started: Instant::now(),
            elapsed: Cell::new(Duration::ZERO),
        }
    }

    /// The field holds `text`, with the first `selected` in it selected.
    pub fn with_text(self, text: &str, selected: &str) -> Self {
        let start = text.find(selected).map_or(0, |at| text[..at].encode_utf16().count());
        *self.text.borrow_mut() = text.into();
        self.selection.set((start, selected.encode_utf16().count()));
        self
    }

    /// A field in web content: written by pasting or typing, never by Accessibility.
    pub fn web(self) -> Self {
        self.web.set(true);
        self
    }

    /// A web field with formatting a plain-text replace would lose.
    pub fn formatted(self) -> Self {
        self.formatted.set(true);
        self.web()
    }

    /// A Chromium-based app.
    pub fn chromium(self) -> Self {
        self.chromium.set(true);
        self
    }

    /// The app reports `AXValue` without empty paragraphs, as Chromium's rich fields do.
    pub fn dropping_empty_paragraphs(self) -> Self {
        self.drops_empty_paragraphs.set(true);
        self
    }

    /// The field's selected text can't be written through Accessibility.
    pub fn not_settable(self) -> Self {
        self.settable.set(false);
        self
    }

    pub fn writes_are(self, write: Write) -> Self {
        self.write.set(write);
        self
    }

    pub fn pastes(self, pastes: Pastes) -> Self {
        self.pastes.set(pastes);
        self
    }

    /// The app accepts every request to move its selection and leaves it where it was.
    pub fn keeping_its_selection(self) -> Self {
        self.moves_selection.set(false);
        self
    }

    pub fn untrusted(self) -> Self {
        self.trusted.set(false);
        self
    }

    /// Runs once, right after the next paste reaches the app.
    pub fn after_paste(&self, act: impl FnOnce(&World) + 'static) {
        *self.after_paste.borrow_mut() = Some(Box::new(act));
    }

    /// Runs once, right after the next Accessibility write.
    pub fn after_write(&self, act: impl FnOnce(&World) + 'static) {
        *self.after_write.borrow_mut() = Some(Box::new(act));
    }

    /// What SuperEditr reads from the field as it is now.
    pub fn capture(&self) -> Capture {
        let (start, length) = self.selection.get();
        Capture {
            pid: PID,
            bundle_id: self.bundle_id.clone(),
            app_name: "QuillPoint".into(),
            text: self.selected_now(),
            bounds: None,
            editable: true,
            web: self.web.get(),
            element: None,
            range: Some((start as isize, length as isize)),
            playground: false,
            code: Vec::new(),
            markers: Vec::new(),
            formatted: self.formatted.get(),
        }
    }

    fn services(&self) -> Services<'_, World> {
        Services { desktop: self, pasteboard: self, clock: self }
    }

    /// Replaces the selection with a rewrite.
    pub fn replace(&self, capture: &Capture, text: &str) -> Result<ReplaceMethod, PlatformError> {
        replace_with(&self.services(), capture, Some(self), text, false)
    }

    /// Replaces the selection with a proofread, which may be applied in place.
    pub fn proofread(&self, capture: &Capture, text: &str) -> Result<ReplaceMethod, PlatformError> {
        replace_with(&self.services(), capture, Some(self), text, true)
    }

    /// Replaces with no field to write into, as for text that was copied.
    pub fn replace_without_a_field(&self, capture: &Capture, text: &str) -> Result<ReplaceMethod, PlatformError> {
        replace_with(&self.services(), capture, None, text, false)
    }

    /// The user selects UTF-16 units `start..start + length`.
    pub fn select_now(&self, start: usize, length: usize) {
        self.selection.set((start, length));
    }

    /// Focus moves to another field or app.
    pub fn focus_elsewhere(&self) {
        self.focused.set(false);
    }

    /// The user copies `text` in another app.
    pub fn user_copies(&self, text: &str) {
        *self.clipboard.borrow_mut() = (text.into(), None);
        self.change_count.set(self.change_count.get() + 1);
    }

    pub fn field_text(&self) -> String {
        self.text.borrow().clone()
    }

    pub fn clipboard_text(&self) -> String {
        self.clipboard.borrow().0.clone()
    }

    pub fn clipboard_html(&self) -> Option<String> {
        self.clipboard.borrow().1.clone()
    }

    /// The pastes and typing the app received.
    pub fn keys_sent(&self) -> Vec<String> {
        self.keys.borrow().clone()
    }

    /// The text of each Accessibility write.
    pub fn writes(&self) -> Vec<String> {
        self.writes.borrow().clone()
    }

    /// Each fix SuperEditr put on the clipboard: its plain text and HTML.
    pub fn put_on_clipboard(&self) -> Vec<Clip> {
        self.put_on_clipboard.borrow().clone()
    }

    /// How long SuperEditr waited, by the pretend clock.
    pub fn waited(&self) -> Duration {
        self.elapsed.get()
    }

    fn selected_now(&self) -> String {
        let (start, length) = self.selection.get();
        let units: Vec<u16> = self.text.borrow().encode_utf16().collect();
        String::from_utf16_lossy(&units[start.min(units.len())..(start + length).min(units.len())])
    }

    /// Puts `text` in place of the selection and the caret after it, as typing or pasting does.
    fn type_over_selection(&self, text: &str) {
        let (start, length) = self.selection.get();
        let mut units: Vec<u16> = self.text.borrow().encode_utf16().collect();
        let inserted: Vec<u16> = text.encode_utf16().collect();
        let end = (start + length).min(units.len());
        units.splice(start..end, inserted.iter().copied());
        *self.text.borrow_mut() = String::from_utf16_lossy(&units);
        self.selection.set((start + inserted.len(), 0));
    }

    fn run(&self, hook: &RefCell<Option<Hook>>) {
        let act = hook.borrow_mut().take();
        if let Some(act) = act {
            act(self);
        }
    }
}

impl Field for World {
    fn value(&self) -> Option<String> {
        let text = self.field_text();
        Some(if self.drops_empty_paragraphs.get() { text.replace("\n\n", "\n") } else { text })
    }

    fn selected(&self) -> Option<String> {
        Some(self.selected_now())
    }

    fn selection(&self) -> Option<String> {
        Some(self.selected_now()).filter(|text| !text.trim().is_empty())
    }

    fn selected_range(&self) -> Option<(isize, isize)> {
        let (start, length) = self.selection.get();
        Some((start as isize, length as isize))
    }

    fn web_text(&self) -> Option<String> {
        self.drops_empty_paragraphs.get().then(|| self.field_text())
    }

    fn can_set_selected_text(&self) -> bool {
        self.settable.get()
    }

    fn set_selected_text(&self, text: &str) -> bool {
        self.writes.borrow_mut().push(text.into());
        match self.write.get() {
            Write::Applies => self.type_over_selection(text),
            Write::Ignored => {}
            Write::Garbled => {
                let mut garbled = text.to_string();
                garbled.pop();
                self.type_over_selection(&garbled);
            }
        }
        self.run(&self.after_write);
        true
    }

    fn select(&self, start: usize, length: usize) -> bool {
        let units = self.field_text().encode_utf16().count();
        if self.moves_selection.get() && start + length <= units {
            self.selection.set((start, length));
        }
        true
    }

    fn has_focus(&self, pid: i32) -> bool {
        self.focused.get() && pid == PID
    }

    fn formatting(&self) -> Formatting {
        Formatting { code: Vec::new(), markers: Vec::new(), any: false }
    }
}

impl Desktop for World {
    fn trusted(&self) -> bool {
        self.trusted.get()
    }

    fn chromium_based(&self, _bundle_id: &str, _pid: i32) -> bool {
        self.chromium.get()
    }

    fn paste(&self, _pid: i32) -> &'static str {
        self.keys.borrow_mut().push("⌘V".into());
        let lands = match self.pastes.get() {
            Pastes::Yes => true,
            Pastes::Never => false,
            Pastes::AfterDropping(0) => true,
            Pastes::AfterDropping(n) => {
                self.pastes.set(Pastes::AfterDropping(n - 1));
                false
            }
        };
        if lands {
            let text = self.clipboard_text();
            self.type_over_selection(&text);
        }
        self.run(&self.after_paste);
        "⌘V to the app's process"
    }

    fn type_text(&self, _pid: i32, text: &str) -> bool {
        self.keys.borrow_mut().push(format!("typed {text:?}"));
        self.type_over_selection(text);
        true
    }

    fn window_state(&self, _pid: i32) -> String {
        "front true".into()
    }
}

impl Pasteboard for World {
    type Snapshot = Clip;

    fn snapshot(&self) -> Clip {
        self.clipboard.borrow().clone()
    }

    fn write_transient(&self, text: &str, html: Option<&str>) -> isize {
        let clip: Clip = (text.into(), html.map(str::to_string));
        self.put_on_clipboard.borrow_mut().push(clip.clone());
        *self.clipboard.borrow_mut() = clip;
        self.change_count.set(self.change_count.get() + 1);
        self.change_count.get()
    }

    fn change_count(&self) -> isize {
        self.change_count.get()
    }

    /// Restores at once: the pretend app has nothing left to read.
    fn restore_soon(&self, snapshot: Clip, ours: isize) {
        if self.change_count.get() == ours {
            *self.clipboard.borrow_mut() = snapshot;
            self.change_count.set(ours + 1);
        }
    }
}

impl Clock for World {
    fn now(&self) -> Instant {
        self.started + self.elapsed.get()
    }

    fn sleep(&self, wait: Duration) {
        self.elapsed.set(self.elapsed.get() + wait);
    }
}
