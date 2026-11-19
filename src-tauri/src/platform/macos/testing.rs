//! A pretend Mac for testing the capture and replace chains: one app in front with a window, one
//! text field in it and a selection in that field, the app's Copy and Cut menu items, the general
//! pasteboard, key events to the app, and a clock that moves only while SuperEditr waits, so a test
//! of a one-second wait takes no time.

use super::Capture;
use super::capture::{Read, capture_with};
use super::replace::replace_with;
use super::seam::{App, Clock, Desktop, Field, Pasteboard, Services};
use super::text::Formatting;
use crate::platform::{PlatformError, Rect, ReplaceMethod};
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

/// The test app's process.
const PID: i32 = 42;

/// The app's elements, by index: the app itself, its window and the text field. Elements added
/// later come after these.
const APP: usize = 0;
const WINDOW: usize = 1;
const FIELD: usize = 2;

/// Where the field says its selection is on screen, and where the field itself is.
pub const SELECTION_BOUNDS: Rect = Rect { x: 120.0, y: 300.0, width: 90.0, height: 18.0 };
pub const MARKER_BOUNDS: Rect = Rect { x: 130.0, y: 310.0, width: 70.0, height: 16.0 };
pub const FIELD_FRAME: Rect = Rect { x: 100.0, y: 280.0, width: 600.0, height: 200.0 };

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

/// What the app does when asked to copy its selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Copies {
    /// Puts the selected text on the clipboard at once.
    Yes,
    /// Puts it there after this long, as a busy Electron app does.
    After(Duration),
    /// Empties the clipboard at once and puts the text there after this long. An app writes a copy
    /// in those two steps, and the clipboard's change count moves only at the first.
    EmptiesThenAfter(Duration),
    /// Empties the clipboard and puts no plain text there, and after this long something else
    /// copies "Brynlee's later copy".
    NoTextThenAnother(Duration),
    /// Puts the selected text on the clipboard at once, and something else copies "Brynlee's later
    /// copy" as soon as the change has been seen once.
    ThenAnotherAtOnce,
    /// Empties the clipboard and puts no plain text there, as copying a picture does.
    NoText,
}

/// A menu item of the app's, such as Edit ▸ Copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Enabled,
    Disabled,
    Missing,
}

type Hook = Box<dyn FnOnce(&World)>;

/// The clipboard's plain text and HTML.
type Clip = (String, Option<String>);

/// One of the app's elements, as Accessibility describes it.
struct Node {
    role: String,
    subrole: Option<String>,
    parent: Option<usize>,
}

impl Node {
    fn new(role: &str, parent: Option<usize>) -> Self {
        Node { role: role.into(), subrole: None, parent }
    }
}

pub struct World {
    bundle_id: String,
    /// The app's elements, by index.
    nodes: RefCell<Vec<Node>>,
    /// The element with keyboard focus.
    focus: Cell<Option<usize>>,
    /// Whether an app is in front at all.
    in_front: Cell<bool>,
    /// The field's whole text, as it really is.
    text: RefCell<String>,
    /// The selection: where it starts and how long it is, in UTF-16 units.
    selection: Cell<(usize, usize)>,
    trusted: Cell<bool>,
    browser: Cell<bool>,
    chromium: Cell<bool>,
    web: Cell<bool>,
    formatted: Cell<bool>,
    settable: Cell<bool>,
    editable_ancestor: Cell<bool>,
    /// The field reports no selection to Accessibility, though the user sees one and can copy it.
    hides_selection: Cell<bool>,
    /// Until the pretend clock reaches this, the app shows no focused element: its web view is
    /// still building its accessibility tree.
    tree_ready: Cell<Duration>,
    asked_for_tree: Cell<usize>,
    formatting_reads: Cell<usize>,
    selection_bounds: Cell<Option<Rect>>,
    marker_bounds: Cell<Option<Rect>>,
    copy_item: Cell<Item>,
    cut_item: Cell<Item>,
    copies: Cell<Copies>,
    /// A copy on its way to the clipboard: when it lands, by the pretend clock, and its text.
    pending_copy: RefCell<Option<(Duration, String)>>,
    /// Text on its way to a clipboard the app has already emptied.
    pending_text: RefCell<Option<(Duration, String)>>,
    /// Something else copies right after the change count is next read.
    another_after_count: Cell<bool>,
    /// The clipboard was emptied and holds no plain text yet.
    no_text: Cell<bool>,
    write: Cell<Write>,
    moves_selection: Cell<bool>,
    /// Every request to move the selection fails.
    refuses_selection: Cell<bool>,
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
    /// focused, Copy and Cut in its Edit menu, and the user's own copy on the clipboard.
    pub fn new(bundle_id: &str) -> Self {
        World {
            bundle_id: bundle_id.into(),
            nodes: RefCell::new(vec![
                Node::new("AXApplication", None),
                Node::new("AXWindow", Some(APP)),
                Node::new("AXTextArea", Some(WINDOW)),
            ]),
            focus: Cell::new(Some(FIELD)),
            in_front: Cell::new(true),
            text: RefCell::default(),
            selection: Cell::new((0, 0)),
            trusted: Cell::new(true),
            browser: Cell::new(false),
            chromium: Cell::new(false),
            web: Cell::new(false),
            formatted: Cell::new(false),
            settable: Cell::new(true),
            editable_ancestor: Cell::new(false),
            hides_selection: Cell::new(false),
            tree_ready: Cell::new(Duration::ZERO),
            asked_for_tree: Cell::new(0),
            formatting_reads: Cell::new(0),
            selection_bounds: Cell::new(Some(SELECTION_BOUNDS)),
            marker_bounds: Cell::new(Some(MARKER_BOUNDS)),
            copy_item: Cell::new(Item::Enabled),
            cut_item: Cell::new(Item::Enabled),
            copies: Cell::new(Copies::Yes),
            pending_copy: RefCell::default(),
            pending_text: RefCell::default(),
            another_after_count: Cell::new(false),
            no_text: Cell::new(false),
            write: Cell::new(Write::Applies),
            moves_selection: Cell::new(true),
            refuses_selection: Cell::new(false),
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

    /// The field holds `marked` without its `|`, with the caret where the `|` is and nothing selected.
    pub fn with_caret(self, marked: &str) -> Self {
        let at = marked.find('|').expect("a caret");
        *self.text.borrow_mut() = marked.replacen('|', "", 1);
        self.selection.set((marked[..at].encode_utf16().count(), 0));
        self
    }

    /// A field in web content, inside a web area: written by pasting or typing, never by
    /// Accessibility.
    pub fn web(self) -> Self {
        self.web.set(true);
        let mut nodes = self.nodes.borrow_mut();
        nodes.push(Node::new("AXWebArea", Some(WINDOW)));
        nodes[FIELD].parent = Some(nodes.len() - 1);
        drop(nodes);
        self
    }

    /// A web field with formatting a plain-text replace would lose.
    pub fn formatted(self) -> Self {
        self.formatted.set(true);
        self.web()
    }

    /// A web browser, which isn't built on Chromium.
    pub fn browser(self) -> Self {
        self.browser.set(true);
        self
    }

    /// A Chromium-based app.
    pub fn chromium(self) -> Self {
        self.chromium.set(true);
        self
    }

    /// The field calls itself `role`, such as `AXButton` or `AXWebArea`.
    pub fn role(self, role: &str) -> Self {
        self.nodes.borrow_mut()[FIELD].role = role.into();
        self
    }

    /// A password field.
    pub fn password(self) -> Self {
        self.nodes.borrow_mut()[FIELD].subrole = Some("AXSecureTextField".into());
        self
    }

    /// Focus is on an element inside the field, as Chromium reports it, rather than on the field.
    pub fn focus_inside_the_field(self) -> Self {
        let mut nodes = self.nodes.borrow_mut();
        nodes.push(Node::new("AXStaticText", Some(FIELD)));
        self.focus.set(Some(nodes.len() - 1));
        drop(nodes);
        self
    }

    /// Focus is `depth` elements down inside the field.
    pub fn focus_nested(self, depth: usize) -> Self {
        let mut nodes = self.nodes.borrow_mut();
        let mut parent = FIELD;
        for _ in 0..depth {
            nodes.push(Node::new("AXGroup", Some(parent)));
            parent = nodes.len() - 1;
        }
        self.focus.set(Some(parent));
        drop(nodes);
        self
    }

    /// The field reports an editable ancestor, as a web editor does.
    pub fn with_editable_ancestor(self) -> Self {
        self.editable_ancestor.set(true);
        self
    }

    /// The app shows the selection but hides it from Accessibility.
    pub fn hiding_its_selection(self) -> Self {
        self.hides_selection.set(true);
        self
    }

    /// The app shows no focused element until `wait` has passed.
    pub fn building_its_tree_for(self, wait: Duration) -> Self {
        self.tree_ready.set(wait);
        self
    }

    /// No app is in front.
    pub fn with_nothing_in_front(self) -> Self {
        self.in_front.set(false);
        self
    }

    /// Where the field says its selection is, by range and by text marker.
    pub fn with_bounds(self, range: Option<Rect>, marker: Option<Rect>) -> Self {
        self.selection_bounds.set(range);
        self.marker_bounds.set(marker);
        self
    }

    pub fn copy_item(self, item: Item) -> Self {
        self.copy_item.set(item);
        self
    }

    pub fn cut_item(self, item: Item) -> Self {
        self.cut_item.set(item);
        self
    }

    pub fn copies(self, copies: Copies) -> Self {
        self.copies.set(copies);
        self
    }

    /// The field's value and selected text can't be written through Accessibility.
    pub fn not_settable(self) -> Self {
        self.settable.set(false);
        self
    }

    /// The app reports `AXValue` without empty paragraphs, as Chromium's rich fields do.
    pub fn dropping_empty_paragraphs(self) -> Self {
        self.drops_empty_paragraphs.set(true);
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

    /// The app refuses every request to move its selection, as a field whose selected range can't
    /// be set does.
    pub fn refusing_selection_changes(self) -> Self {
        self.refuses_selection.set(true);
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

    fn services(&self) -> Services<'_, World, World> {
        Services { desktop: self, pasteboard: self, clock: self }
    }

    /// The text field.
    pub fn field(&self) -> Ui<'_> {
        Ui { world: self, id: FIELD }
    }

    /// Reads the selection, by a shortcut when `allow_clipboard` is set and automatically otherwise.
    pub fn read(&self, allow_clipboard: bool, select_sentence: bool) -> Result<Read<Ui<'_>>, PlatformError> {
        capture_with(&self.services(), allow_clipboard, select_sentence)
    }

    /// Reads the selection for a shortcut, which may copy.
    pub fn read_by_shortcut(&self) -> Result<Read<Ui<'_>>, PlatformError> {
        self.read(true, false)
    }

    /// Reads the selection for an automatic suggestion, which never copies.
    pub fn read_automatically(&self) -> Result<Read<Ui<'_>>, PlatformError> {
        self.read(false, false)
    }

    /// Replaces the selection with a rewrite.
    pub fn replace(&self, capture: &Capture, text: &str) -> Result<ReplaceMethod, PlatformError> {
        replace_with(&self.services(), capture, Some(&self.field()), text, false)
    }

    /// Replaces the selection with a proofread, which may be applied in place.
    pub fn proofread(&self, capture: &Capture, text: &str) -> Result<ReplaceMethod, PlatformError> {
        replace_with(&self.services(), capture, Some(&self.field()), text, true)
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
        self.focus.set(None);
    }

    /// The user copies `text` in another app.
    pub fn user_copies(&self, text: &str) {
        *self.clipboard.borrow_mut() = (text.into(), None);
        self.no_text.set(false);
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

    /// The pastes, copies and typing the app received.
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

    /// How many times SuperEditr asked the app for its accessibility tree.
    pub fn asked_for_tree(&self) -> usize {
        self.asked_for_tree.get()
    }

    /// How many times SuperEditr looked for code and other formatting in the field.
    pub fn formatting_reads(&self) -> usize {
        self.formatting_reads.get()
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

    /// The app copies its selection, as it does for its Copy menu item or ⌘C.
    fn copy(&self) {
        let text = self.selected_now();
        match self.copies.get() {
            Copies::Yes => self.user_copies(&text),
            Copies::After(wait) => *self.pending_copy.borrow_mut() = Some((self.elapsed.get() + wait, text)),
            Copies::EmptiesThenAfter(wait) => {
                self.empty_clipboard();
                *self.pending_text.borrow_mut() = Some((self.elapsed.get() + wait, text));
            }
            Copies::ThenAnotherAtOnce => {
                self.user_copies(&text);
                self.another_after_count.set(true);
            }
            Copies::NoTextThenAnother(wait) => {
                self.empty_clipboard();
                *self.pending_copy.borrow_mut() = Some((self.elapsed.get() + wait, "Brynlee's later copy".into()));
            }
            Copies::NoText => self.empty_clipboard(),
        }
    }

    fn empty_clipboard(&self) {
        *self.clipboard.borrow_mut() = (String::new(), None);
        self.no_text.set(true);
        self.change_count.set(self.change_count.get() + 1);
    }

    /// A copy on its way lands once its time has come.
    fn land_copy(&self) {
        let due = self.pending_copy.borrow().as_ref().is_some_and(|(at, _)| self.elapsed.get() >= *at);
        if due && let Some((_, text)) = self.pending_copy.borrow_mut().take() {
            self.user_copies(&text);
        }
        // The second step of a copy: the text, under the change count the first step left.
        let due = self.pending_text.borrow().as_ref().is_some_and(|(at, _)| self.elapsed.get() >= *at);
        if due && let Some((_, text)) = self.pending_text.borrow_mut().take() {
            *self.clipboard.borrow_mut() = (text, None);
            self.no_text.set(false);
        }
    }

    fn run(&self, hook: &RefCell<Option<Hook>>) {
        let act = hook.borrow_mut().take();
        if let Some(act) = act {
            act(self);
        }
    }
}

/// One of the pretend app's elements: the app itself, its window, the text field, or something
/// around or inside the field.
#[derive(Clone, Copy)]
pub struct Ui<'a> {
    world: &'a World,
    id: usize,
}

impl std::fmt::Debug for Ui<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "element {}", self.id)
    }
}

impl Ui<'_> {
    fn is_field(&self) -> bool {
        self.id == FIELD
    }

    /// The field's text and selection, while Accessibility shows them.
    fn field_selection(&self) -> Option<String> {
        (self.is_field() && !self.world.hides_selection.get()).then(|| self.world.selected_now())
    }
}

impl Field for Ui<'_> {
    fn value(&self) -> Option<String> {
        let text = self.world.field_text();
        self.is_field().then(|| if self.world.drops_empty_paragraphs.get() { text.replace("\n\n", "\n") } else { text })
    }

    fn selected(&self) -> Option<String> {
        self.field_selection()
    }

    fn selection(&self) -> Option<String> {
        self.field_selection().filter(|text| !text.trim().is_empty())
    }

    fn selected_range(&self) -> Option<(isize, isize)> {
        let (start, length) = self.world.selection.get();
        (self.is_field() && !self.world.hides_selection.get()).then_some((start as isize, length as isize))
    }

    fn web_text(&self) -> Option<String> {
        (self.is_field() && self.world.drops_empty_paragraphs.get()).then(|| self.world.field_text())
    }

    fn can_set_selected_text(&self) -> bool {
        self.is_field() && self.world.settable.get()
    }

    fn set_selected_text(&self, text: &str) -> bool {
        let world = self.world;
        if !self.is_field() {
            return false;
        }
        world.writes.borrow_mut().push(text.into());
        match world.write.get() {
            Write::Applies => world.type_over_selection(text),
            Write::Ignored => {}
            Write::Garbled => {
                let mut garbled = text.to_string();
                garbled.pop();
                world.type_over_selection(&garbled);
            }
        }
        world.run(&world.after_write);
        true
    }

    fn select(&self, start: usize, length: usize) -> bool {
        let world = self.world;
        let units = world.field_text().encode_utf16().count();
        if world.refuses_selection.get() {
            return false;
        }
        if self.is_field() && world.moves_selection.get() && start + length <= units {
            world.selection.set((start, length));
        }
        self.is_field()
    }

    fn has_focus(&self, pid: i32) -> bool {
        let nodes = self.world.nodes.borrow();
        let mut current = self.world.focus.get();
        while let Some(id) = current {
            if id == self.id {
                return pid == PID;
            }
            current = nodes[id].parent;
        }
        false
    }

    fn formatting(&self) -> Formatting {
        self.world.formatting_reads.set(self.world.formatting_reads.get() + 1);
        Formatting { code: Vec::new(), markers: Vec::new(), any: self.world.formatted.get() }
    }

    fn role(&self) -> Option<String> {
        Some(self.world.nodes.borrow()[self.id].role.clone())
    }

    fn subrole(&self) -> Option<String> {
        self.world.nodes.borrow()[self.id].subrole.clone()
    }

    fn parent(&self) -> Option<Self> {
        let parent = self.world.nodes.borrow()[self.id].parent?;
        Some(Ui { world: self.world, id: parent })
    }

    fn same_as(&self, other: &Self) -> bool {
        std::ptr::eq(self.world, other.world) && self.id == other.id
    }

    fn value_settable(&self) -> bool {
        self.is_field() && self.world.settable.get()
    }

    fn has_editable_ancestor(&self) -> bool {
        self.is_field() && self.world.editable_ancestor.get()
    }

    fn character_count(&self) -> Option<i64> {
        self.is_field().then(|| self.world.field_text().encode_utf16().count() as i64)
    }

    fn bounds_for_range(&self, _location: isize, _length: isize) -> Option<Rect> {
        self.world.selection_bounds.get().filter(|_| self.is_field())
    }

    fn marker_bounds(&self) -> Option<Rect> {
        self.world.marker_bounds.get().filter(|_| self.is_field())
    }

    fn frame(&self) -> Option<Rect> {
        self.is_field().then_some(FIELD_FRAME)
    }
}

impl App for Ui<'_> {
    type Node = Self;

    fn focused(&self) -> Option<Self> {
        let world = self.world;
        if world.elapsed.get() < world.tree_ready.get() {
            return None;
        }
        Some(Ui { world, id: world.focus.get()? })
    }

    /// The app takes the request, and reads wait up to four seconds for its tree, as for a real one.
    fn ask_for_tree(&self, _pid: i32, _bundle_id: &str) -> Instant {
        self.world.asked_for_tree.set(self.world.asked_for_tree.get() + 1);
        self.world.now() + Duration::from_secs(4)
    }

    fn press_menu_item(&self, key: &str) -> Option<bool> {
        if key != "c" {
            return None;
        }
        match self.world.copy_item.get() {
            Item::Missing => None,
            Item::Disabled => Some(false),
            Item::Enabled => {
                self.world.keys.borrow_mut().push("Copy menu item".into());
                self.world.copy();
                Some(true)
            }
        }
    }

    fn cut_enabled(&self) -> bool {
        self.world.cut_item.get() == Item::Enabled
    }
}

impl Desktop for World {
    type App<'a> = Ui<'a>;

    fn trusted(&self) -> bool {
        self.trusted.get()
    }

    fn focused_app(&self) -> Option<(Ui<'_>, i32)> {
        self.in_front.get().then_some((Ui { world: self, id: APP }, PID))
    }

    fn app_identity(&self, _pid: i32) -> (String, String) {
        (self.bundle_id.clone(), "QuillPoint".into())
    }

    fn browser_or_electron(&self, _bundle_id: &str, _pid: i32) -> bool {
        self.browser.get() || self.chromium.get()
    }

    fn chromium_based(&self, _bundle_id: &str, _pid: i32) -> bool {
        self.chromium.get()
    }

    fn post_copy(&self, pid: i32) {
        let how = if pid == 0 { "⌘C keystroke" } else { "⌘C to the app's process" };
        self.keys.borrow_mut().push(how.into());
        self.copy();
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
        self.land_copy();
        let count = self.change_count.get();
        if self.another_after_count.replace(false) {
            self.user_copies("Brynlee's later copy");
        }
        count
    }

    fn read_string(&self) -> Option<String> {
        self.land_copy();
        (!self.no_text.get()).then(|| self.clipboard_text())
    }

    fn restore(&self, snapshot: Clip, ours: isize) {
        if self.change_count.get() == ours {
            *self.clipboard.borrow_mut() = snapshot;
            self.no_text.set(false);
            self.change_count.set(ours + 1);
        }
    }

    /// Restores at once: the pretend app has nothing left to read.
    fn restore_soon(&self, snapshot: Clip, ours: isize) {
        self.restore(snapshot, ours);
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
