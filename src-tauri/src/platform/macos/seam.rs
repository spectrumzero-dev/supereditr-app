//! What reading and writing the selection need from the Mac, as small traits, so the decisions in
//! `capture.rs` and `replace.rs` can be tested against a pretend Mac (`testing.rs`).
//!
//! The traits: the [`Field`] text is read from and written into, the [`App`] in front, the
//! [`Desktop`] that finds the app and takes key events, the [`Pasteboard`], and a [`Clock`] for every
//! wait. [`System`] and [`Element`] are the real ones.

use super::apps::{app_identity, enable_web_accessibility, focused_app, is_browser_or_electron, is_chromium_based};
use super::ax::{AXUIElementCreateApplication, Element, to_rect};
use super::capture::{cut_enabled, press_menu_item};
use super::keys::{Key, post_command_key};
use super::replace::{field_focused, key_window_state, set_selection, type_text};
use super::text::{Formatting, code_in};
use super::{accessibility_trusted, pasteboard};
use crate::platform::Rect;
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use std::time::{Duration, Instant};

/// An element of an app, as Accessibility shows it: the field text is read from and written into,
/// or one of its ancestors. [`Element`] is the real one.
pub(super) trait Field {
    /// `AXValue`: the field's whole text.
    fn value(&self) -> Option<String>;
    /// `AXSelectedText`, exactly as the app reports it.
    fn selected(&self) -> Option<String>;
    /// The selection as a capture reads it ([`Element::selected_text`]), to compare with what was
    /// captured.
    fn selection(&self) -> Option<String>;
    /// `AXSelectedTextRange`: where the selection starts and how long it is, in UTF-16 units.
    fn selected_range(&self) -> Option<(isize, isize)>;
    /// A Chromium rich field's text with its empty paragraphs put back ([`Element::web_text`]).
    fn web_text(&self) -> Option<String>;
    /// Whether the app lets `AXSelectedText` be written.
    fn can_set_selected_text(&self) -> bool;
    /// Writes `text` over the selection through Accessibility. True when the app accepted the write,
    /// which doesn't mean it applied it.
    fn set_selected_text(&self, text: &str) -> bool;
    /// Sets the selection to UTF-16 units `start..start + length`, without waiting.
    fn select(&self, start: usize, length: usize) -> bool;
    /// True while the app `pid` is in front with focus in this field or inside it.
    fn has_focus(&self, pid: i32) -> bool;
    /// The code and other formatting in a web field. Finding it also logs what was found.
    fn formatting(&self) -> Formatting;
    /// `AXRole`.
    fn role(&self) -> Option<String>;
    /// `AXSubrole`.
    fn subrole(&self) -> Option<String>;
    /// `AXParent`.
    fn parent(&self) -> Option<Self>
    where
        Self: Sized;
    /// Whether `other` is this same element.
    fn same_as(&self, other: &Self) -> bool
    where
        Self: Sized;
    /// Whether the app lets `AXValue` be written.
    fn value_settable(&self) -> bool;
    /// Whether the element reports an `AXEditableAncestor`, as web editors do.
    fn has_editable_ancestor(&self) -> bool;
    /// `AXNumberOfCharacters`: the field's whole length.
    fn character_count(&self) -> Option<i64>;
    /// `AXBoundsForRange` of UTF-16 units `location..location + length`, on screen.
    fn bounds_for_range(&self, location: isize, length: isize) -> Option<Rect>;
    /// Where a selection exposed only as a text-marker range is on screen
    /// ([`Element::marker_selection`]).
    fn marker_bounds(&self) -> Option<Rect>;
    /// The element's position and size on screen.
    fn frame(&self) -> Option<Rect>;
}

/// The app in front, as Accessibility shows it.
pub(super) trait App {
    /// Its elements.
    type Node: Field + Clone;
    /// `AXFocusedUIElement`: the element with keyboard focus.
    fn focused(&self) -> Option<Self::Node>;
    /// Asks a web view for its accessibility tree, and returns when reads should stop waiting for
    /// it ([`enable_web_accessibility`]).
    fn ask_for_tree(&self, pid: i32, bundle_id: &str) -> Instant;
    /// Presses the menu item whose shortcut is ⌘ plus `key` alone ([`press_menu_item`]): Some(false)
    /// when it is disabled, None when the app has no such item.
    fn press_menu_item(&self, key: &str) -> Option<bool>;
    /// Whether the app's own Cut menu item is enabled ([`cut_enabled`]).
    fn cut_enabled(&self) -> bool;
}

/// The app in front, what it is, key events to it, and what the log says about its windows.
pub(super) trait Desktop {
    /// The app in front.
    type App<'a>: App
    where
        Self: 'a;
    /// Whether SuperEditr has Accessibility permission.
    fn trusted(&self) -> bool;
    /// The app that owns keyboard focus, and its process.
    fn focused_app(&self) -> Option<(Self::App<'_>, i32)>;
    /// The app `pid`'s bundle ID and name.
    fn app_identity(&self, pid: i32) -> (String, String);
    /// Safari, Firefox, and Chromium browsers and apps.
    fn browser_or_electron(&self, bundle_id: &str, pid: i32) -> bool;
    /// Chrome-family browsers and apps built on Chromium, whose pastes can be dropped.
    fn chromium_based(&self, bundle_id: &str, pid: i32) -> bool;
    /// Sends ⌘C to the app `pid`; with `pid` 0, as a keystroke to whatever app is in front.
    fn post_copy(&self, pid: i32);
    /// Pastes in the app `pid`, and says how, for the log.
    fn paste(&self, pid: i32) -> &'static str;
    /// Types `text` over the app's selection as key events; empty text deletes it.
    fn type_text(&self, pid: i32, text: &str) -> bool;
    /// Which of the app's windows has focus, for the log.
    fn window_state(&self, pid: i32) -> String;
}

/// The general pasteboard.
pub(super) trait Pasteboard {
    /// Everything on the clipboard, to put back after a paste or a copy.
    type Snapshot;
    fn snapshot(&self) -> Self::Snapshot;
    /// Writes the fix, marked transient, with an HTML version when given one. Returns the change
    /// count after the write.
    fn write_transient(&self, text: &str, html: Option<&str>) -> isize;
    fn change_count(&self) -> isize;
    /// The clipboard's plain text.
    fn read_string(&self) -> Option<String>;
    /// Puts `snapshot` back now, unless something wrote to the clipboard since change count `ours`.
    fn restore(&self, snapshot: Self::Snapshot, ours: isize);
    /// Puts `snapshot` back once the app has had a moment to read the fix, unless something else
    /// wrote to the clipboard since change count `ours`.
    fn restore_soon(&self, snapshot: Self::Snapshot, ours: isize);
}

/// The time every wait is measured by.
pub(super) trait Clock {
    fn now(&self) -> Instant;
    fn sleep(&self, wait: Duration);
}

/// What a read or a replace runs against.
pub(super) struct Services<'a, D: Desktop, P: Pasteboard> {
    pub(super) desktop: &'a D,
    pub(super) pasteboard: &'a P,
    pub(super) clock: &'a dyn Clock,
}

/// The real Mac: Accessibility permission, the app in front, key events, the general pasteboard and
/// the clock.
pub(super) struct System;

impl System {
    pub(super) fn services() -> Services<'static, System, System> {
        Services { desktop: &System, pasteboard: &System, clock: &System }
    }
}

impl Field for Element {
    fn value(&self) -> Option<String> {
        self.string("AXValue")
    }

    fn selected(&self) -> Option<String> {
        self.string("AXSelectedText")
    }

    fn selection(&self) -> Option<String> {
        self.selected_text()
    }

    fn selected_range(&self) -> Option<(isize, isize)> {
        self.range("AXSelectedTextRange").map(|range| (range.location, range.length))
    }

    fn web_text(&self) -> Option<String> {
        Element::web_text(self)
    }

    fn can_set_selected_text(&self) -> bool {
        self.settable("AXSelectedText")
    }

    fn set_selected_text(&self, text: &str) -> bool {
        let value = CFString::new(text);
        self.set("AXSelectedText", value.as_CFTypeRef())
    }

    fn select(&self, start: usize, length: usize) -> bool {
        set_selection(self, start, length)
    }

    fn has_focus(&self, pid: i32) -> bool {
        field_focused(pid, self)
    }

    fn formatting(&self) -> Formatting {
        code_in(self)
    }

    fn role(&self) -> Option<String> {
        self.string("AXRole")
    }

    fn subrole(&self) -> Option<String> {
        self.string("AXSubrole")
    }

    fn parent(&self) -> Option<Element> {
        self.element("AXParent")
    }

    fn same_as(&self, other: &Element) -> bool {
        Element::same_as(self, other)
    }

    fn value_settable(&self) -> bool {
        self.settable("AXValue")
    }

    fn has_editable_ancestor(&self) -> bool {
        self.attr("AXEditableAncestor").is_some()
    }

    fn character_count(&self) -> Option<i64> {
        self.number("AXNumberOfCharacters")
    }

    fn bounds_for_range(&self, location: isize, length: isize) -> Option<Rect> {
        Element::bounds_for_range(self, super::ax::CFRange { location, length }).map(to_rect)
    }

    fn marker_bounds(&self) -> Option<Rect> {
        self.marker_selection().and_then(|(_, bounds)| bounds).map(to_rect)
    }

    fn frame(&self) -> Option<Rect> {
        Element::frame(self).map(to_rect)
    }
}

impl App for Element {
    type Node = Element;

    fn focused(&self) -> Option<Element> {
        self.element("AXFocusedUIElement")
    }

    fn ask_for_tree(&self, pid: i32, bundle_id: &str) -> Instant {
        enable_web_accessibility(self, pid, bundle_id)
    }

    fn press_menu_item(&self, key: &str) -> Option<bool> {
        press_menu_item(self, key)
    }

    fn cut_enabled(&self) -> bool {
        cut_enabled(self)
    }
}

impl Desktop for System {
    type App<'a> = Element;

    fn trusted(&self) -> bool {
        accessibility_trusted(false)
    }

    fn focused_app(&self) -> Option<(Element, i32)> {
        focused_app()
    }

    fn app_identity(&self, pid: i32) -> (String, String) {
        app_identity(pid)
    }

    fn browser_or_electron(&self, bundle_id: &str, pid: i32) -> bool {
        is_browser_or_electron(bundle_id, pid)
    }

    fn chromium_based(&self, bundle_id: &str, pid: i32) -> bool {
        is_chromium_based(bundle_id, pid)
    }

    fn post_copy(&self, pid: i32) {
        post_command_key(pid, Key::C);
    }

    /// The app's own Paste menu item first. The app can be left without a key window (after the
    /// panel had focus for an instruction, or the user clicked it); browsers and Electron apps then
    /// drop a posted ⌘V, but a menu item acts on the app's main window.
    fn paste(&self, pid: i32) -> &'static str {
        let app = Element::from_create(unsafe { AXUIElementCreateApplication(pid) });
        if app.as_ref().and_then(|app| press_menu_item(app, "v")) == Some(true) {
            "the app's Paste menu item"
        } else {
            post_command_key(pid, Key::V);
            "⌘V to the app's process"
        }
    }

    fn type_text(&self, pid: i32, text: &str) -> bool {
        type_text(pid, text)
    }

    fn window_state(&self, pid: i32) -> String {
        key_window_state(pid)
    }
}

impl Pasteboard for System {
    type Snapshot = pasteboard::Snapshot;

    fn snapshot(&self) -> pasteboard::Snapshot {
        pasteboard::Snapshot::take()
    }

    fn write_transient(&self, text: &str, html: Option<&str>) -> isize {
        pasteboard::write_transient_string(text, html)
    }

    fn change_count(&self) -> isize {
        pasteboard::change_count()
    }

    fn read_string(&self) -> Option<String> {
        pasteboard::read_string()
    }

    fn restore(&self, snapshot: pasteboard::Snapshot, ours: isize) {
        snapshot.restore_if_unchanged_since(ours);
    }

    /// Gives the app a moment to finish reading the pasteboard. A newer copy from another app
    /// always wins over the restore.
    fn restore_soon(&self, snapshot: pasteboard::Snapshot, ours: isize) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            snapshot.restore_if_unchanged_since(ours);
        });
    }
}

impl Clock for System {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, wait: Duration) {
        std::thread::sleep(wait);
    }
}
