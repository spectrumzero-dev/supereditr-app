//! The focused text field in another app, through AT-SPI, the Linux accessibility bus.
//!
//! AT-SPI is for reading only: the field, its role, whether it's editable, and the selection, and
//! after a paste, whether the new text landed. SuperEditr never writes text through it (its
//! edits break undo in GTK and Qt and do nothing in Firefox); the one change it makes is
//! selecting the sentence before the caret, which is a selection, not an edit.
//!
//! Every call has a time limit, so a frozen app can't hold up a review. Offsets are in characters
//! (Unicode scalar values), as AT-SPI counts them; macOS counts UTF-16 units.

use crate::platform::Rect;
use async_trait::async_trait;
use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::text::TextProxy;
use atspi::{CoordType, Role, State};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use zbus::proxy::CacheProperties;

/// The longest one AT-SPI call may take.
const CALL_LIMIT: Duration = Duration::from_millis(300);
/// The longest a search of an app's tree for its focused field may take, and how many objects it
/// looks at, for an app SuperEditr hasn't seen focus move in.
const SEARCH_LIMIT: Duration = Duration::from_millis(1200);
const SEARCH_NODES: usize = 4000;
/// The longest selection SuperEditr reads, in characters.
const MAX_CHARS: i32 = 50_000;
/// How far back from the caret the sentence before it is looked for.
const SENTENCE_REACH: i32 = 4000;

/// Where a field lives on the accessibility bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldId {
    pub bus: String,
    pub path: String,
}

/// The focused field as read, with its selection.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub id: FieldId,
    pub editable: bool,
    /// A password field: its text isn't read.
    pub password: bool,
    /// The selection's character offsets; equal when nothing is selected.
    pub start: i32,
    pub end: i32,
    pub selected: String,
    pub caret: i32,
    /// The selection in "screen" coordinates. On Wayland no app knows where its window is, so a
    /// Wayland app reports these relative to its window (Chromium, Qt); an XWayland app's are
    /// global. Firefox reports none.
    pub bounds: Option<Rect>,
    /// The selection in window coordinates, asked for only when there are no screen ones: GTK 4
    /// refuses screen coordinates but reports these correctly (Chromium's are relative to the page,
    /// not the window, so they come second).
    pub window_bounds: Option<Rect>,
}

impl Field {
    pub fn has_selection(&self) -> bool {
        self.end > self.start && !self.selected.is_empty()
    }

    /// The selection holds embedded objects (U+FFFC), such as Obsidian's lines, so its text isn't
    /// all there.
    pub fn partial(&self) -> bool {
        self.selected.contains('\u{FFFC}')
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("the accessibility bus isn’t available: {0}")]
    Unavailable(String),
    #[error("the app didn’t answer in time")]
    Timeout,
    #[error("the selection is too long")]
    TooLong,
    #[error("{0}")]
    Failed(String),
}

impl From<zbus::Error> for ReadError {
    fn from(error: zbus::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

impl From<zbus::fdo::Error> for ReadError {
    fn from(error: zbus::fdo::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

impl From<atspi::AtspiError> for ReadError {
    fn from(error: atspi::AtspiError) -> Self {
        Self::Failed(error.to_string())
    }
}

/// Reading fields. [`Atspi`] is the real one; tests use a fake.
#[async_trait]
pub trait Fields: Send + Sync {
    /// The focused field in one of this process's windows; `None` when it has none that exposes
    /// text, or doesn't use AT-SPI at all.
    async fn focused(&self, pid: i32) -> Result<Option<Field>, ReadError>;
    /// A field's text between two character offsets.
    async fn text(&self, id: &FieldId, start: i32, end: i32) -> Result<String, ReadError>;
    /// Selects the sentence before the caret and reads the field again; `None` when there's no
    /// sentence there.
    async fn select_sentence(&self, field: &Field) -> Result<Option<Field>, ReadError>;
}

async fn limit<T, E: Into<ReadError>>(call: impl Future<Output = Result<T, E>>) -> Result<T, ReadError> {
    match tokio::time::timeout(CALL_LIMIT, call).await {
        Ok(result) => result.map_err(Into::into),
        Err(_) => Err(ReadError::Timeout),
    }
}

/// The session's accessibility bus.
#[derive(Default)]
pub struct Atspi {
    connection: tokio::sync::OnceCell<atspi::AccessibilityConnection>,
    /// The object that most recently reported getting focus in each app (by bus name), from
    /// AT-SPI's events.
    last_focus: Mutex<HashMap<String, FieldId>>,
    /// Each app's bus name and process.
    pids: Mutex<HashMap<String, i32>>,
}

impl Atspi {
    async fn connection(&self) -> Result<&atspi::AccessibilityConnection, ReadError> {
        self.connection
            .get_or_try_init(|| async {
                match tokio::time::timeout(Duration::from_secs(2), atspi::AccessibilityConnection::new()).await {
                    Ok(connection) => connection.map_err(|e| ReadError::Unavailable(e.to_string())),
                    Err(_) => Err(ReadError::Timeout),
                }
            })
            .await
    }

    /// Follows focus as apps report it, so a capture usually needs no search. Runs until the bus
    /// goes away.
    pub async fn follow_focus(&self) -> Result<(), ReadError> {
        use atspi::events::object::StateChangedEvent;
        use atspi::events::{Event, ObjectEvents};
        use futures_util::StreamExt;
        let connection = self.connection().await?;
        connection.register_event::<StateChangedEvent>().await?;
        let mut events = std::pin::pin!(connection.event_stream());
        while let Some(event) = events.next().await {
            let Ok(Event::Object(ObjectEvents::StateChanged(changed))) = event else { continue };
            if changed.state != State::Focused || !changed.enabled {
                continue;
            }
            let (Some(bus), path) = (changed.item.name_as_str(), changed.item.path_as_str()) else { continue };
            let id = FieldId { bus: bus.to_string(), path: path.to_string() };
            self.last_focus.lock().unwrap().insert(id.bus.clone(), id);
        }
        Ok(())
    }

    async fn accessible(&self, bus: &str, path: &str) -> Result<AccessibleProxy<'static>, ReadError> {
        let connection = self.connection().await?.connection();
        Ok(AccessibleProxy::builder(connection)
            .destination(bus.to_string())?
            .path(path.to_string())?
            .cache_properties(CacheProperties::No)
            .build()
            .await?)
    }

    async fn text_proxy(&self, id: &FieldId) -> Result<TextProxy<'static>, ReadError> {
        let connection = self.connection().await?.connection();
        Ok(TextProxy::builder(connection)
            .destination(id.bus.clone())?
            .path(id.path.clone())?
            .cache_properties(CacheProperties::No)
            .build()
            .await?)
    }

    async fn pid_of(&self, bus: &str) -> Result<i32, ReadError> {
        if let Some(pid) = self.pids.lock().unwrap().get(bus) {
            return Ok(*pid);
        }
        let connection = self.connection().await?.connection();
        let dbus = zbus::fdo::DBusProxy::new(connection).await?;
        let name = zbus::names::BusName::try_from(bus.to_string()).map_err(|e| ReadError::Failed(e.to_string()))?;
        let pid = limit(dbus.get_connection_unix_process_id(name)).await? as i32;
        self.pids.lock().unwrap().insert(bus.to_string(), pid);
        Ok(pid)
    }

    /// The bus name of the app running as `pid`.
    async fn app_bus(&self, pid: i32) -> Result<Option<String>, ReadError> {
        let root = self.accessible("org.a11y.atspi.Registry", "/org/a11y/atspi/accessible/root").await?;
        for app in limit(root.get_children()).await? {
            let Some(bus) = app.name_as_str() else { continue };
            if self.pid_of(bus).await.is_ok_and(|p| p == pid) {
                return Ok(Some(bus.to_string()));
            }
        }
        Ok(None)
    }

    /// The object that has focus in the app, by a search of its showing objects, active window
    /// first. Chromium marks the page focused as well as the field in it, so the deepest focused
    /// object wins.
    async fn search_focus(&self, bus: &str) -> Result<Option<FieldId>, ReadError> {
        let started = Instant::now();
        let app = self.accessible(bus, "/org/a11y/atspi/accessible/root").await?;
        let mut windows = Vec::new();
        for window in limit(app.get_children()).await? {
            let path = window.path_as_str().to_string();
            let proxy = self.accessible(bus, &path).await?;
            let active = limit(proxy.get_state()).await.is_ok_and(|s| s.contains(State::Active));
            windows.push((active, path));
        }
        // Active windows last, so they're popped first.
        windows.sort_by_key(|(active, _)| *active);
        let mut stack: Vec<String> = windows.into_iter().map(|(_, path)| path).collect();
        let mut seen = 0;
        let mut found = None;
        while let Some(path) = stack.pop() {
            seen += 1;
            if seen > SEARCH_NODES || started.elapsed() > SEARCH_LIMIT {
                return if found.is_some() { Ok(found) } else { Err(ReadError::Timeout) };
            }
            let node = self.accessible(bus, &path).await?;
            let Ok(states) = limit(node.get_state()).await else { continue };
            if states.contains(State::Defunct) || !states.contains(State::Showing) {
                continue;
            }
            if states.contains(State::Focused) {
                // Only this object's descendants can hold a deeper focus.
                stack.clear();
                found = Some(FieldId { bus: bus.to_string(), path: path.clone() });
            }
            if let Ok(children) = limit(node.get_children()).await {
                stack.extend(children.into_iter().rev().map(|child| child.path_as_str().to_string()));
            }
        }
        Ok(found)
    }

    async fn read(&self, id: &FieldId) -> Result<Option<Field>, ReadError> {
        let node = self.accessible(&id.bus, &id.path).await?;
        let states = limit(node.get_state()).await?;
        if !states.contains(State::Focused) || states.contains(State::Defunct) {
            return Ok(None);
        }
        let password = limit(node.get_role()).await? == Role::PasswordText;
        let editable = states.contains(State::Editable);
        let blank = Field {
            id: id.clone(),
            editable,
            password,
            start: 0,
            end: 0,
            selected: String::new(),
            caret: 0,
            bounds: None,
            window_bounds: None,
        };
        if password {
            return Ok(Some(blank));
        }
        let interfaces = limit(node.get_interfaces()).await?;
        if !interfaces.contains(atspi::Interface::Text) {
            return Ok(None);
        }
        let text = self.text_proxy(id).await?;
        let caret = limit(text.caret_offset()).await.unwrap_or(0);
        let (start, end) = match limit(text.get_n_selections()).await? {
            0 => (caret, caret),
            _ => limit(text.get_selection(0)).await?,
        };
        let (start, end) = (start.min(end).max(0), end.max(start));
        if end - start > MAX_CHARS {
            return Err(ReadError::TooLong);
        }
        let selected = if end > start { limit(text.get_text(start, end)).await? } else { String::new() };
        // Where the selection is; with nothing selected, where the caret is.
        let extents = |coordinates| {
            let text = &text;
            async move {
                match limit(text.get_range_extents(start, end.max(start + 1), coordinates)).await {
                    Ok((x, y, width, height)) if width > 0 && height > 0 && x >= 0 && y >= 0 && (x, y) != (0, 0) => {
                        Some(Rect { x: x as f64, y: y as f64, width: width as f64, height: height as f64 })
                    }
                    _ => None,
                }
            }
        };
        let bounds = extents(CoordType::Screen).await;
        let window_bounds = if bounds.is_none() { extents(CoordType::Window).await } else { None };
        Ok(Some(Field { start, end, selected, caret, bounds, window_bounds, ..blank }))
    }
}

#[async_trait]
impl Fields for Atspi {
    async fn focused(&self, pid: i32) -> Result<Option<Field>, ReadError> {
        let cached: Vec<FieldId> = self.last_focus.lock().unwrap().values().cloned().collect();
        for id in cached {
            if self.pid_of(&id.bus).await.is_ok_and(|p| p == pid)
                && let Ok(Some(field)) = self.read(&id).await
            {
                return Ok(Some(field));
            }
        }
        let Some(bus) = self.app_bus(pid).await? else { return Ok(None) };
        match self.search_focus(&bus).await? {
            Some(id) => self.read(&id).await,
            None => Ok(None),
        }
    }

    async fn text(&self, id: &FieldId, start: i32, end: i32) -> Result<String, ReadError> {
        let text = self.text_proxy(id).await?;
        limit(text.get_text(start, end)).await
    }

    async fn select_sentence(&self, field: &Field) -> Result<Option<Field>, ReadError> {
        let from = (field.caret - SENTENCE_REACH).max(0);
        let before = self.text(&field.id, from, field.caret).await?;
        let Some((start, end)) = sentence_before(&before, before.chars().count()) else { return Ok(None) };
        let (start, end) = (from + start as i32, from + end as i32);
        let text = self.text_proxy(&field.id).await?;
        let selected = match limit(text.get_n_selections()).await? {
            0 => limit(text.add_selection(start, end)).await?,
            _ => limit(text.set_selection(0, start, end)).await?,
        };
        if !selected {
            return Ok(None);
        }
        self.read(&field.id).await
    }
}

/// The byte range of a character range in `text`; `None` when it runs past the end.
pub fn char_range_to_bytes(text: &str, start: usize, end: usize) -> Option<std::ops::Range<usize>> {
    let byte = |chars: usize| {
        if chars == text.chars().count() { Some(text.len()) } else { text.char_indices().nth(chars).map(|(i, _)| i) }
    };
    let (start, end) = (byte(start)?, byte(end)?);
    (start <= end).then_some(start..end)
}

/// The sentence that ends at or contains the caret, skipping whitespace just before it, as a
/// character range in `text` without its surrounding whitespace. Sentences follow Unicode's rules
/// (UAX #29), which keep "e.g. the" in one sentence.
pub fn sentence_before(text: &str, caret: usize) -> Option<(usize, usize)> {
    use unicode_segmentation::UnicodeSegmentation;
    let caret = char_range_to_bytes(text, 0, caret)?.end;
    let head = &text[..caret];
    let last = head.char_indices().rev().find(|(_, c)| !c.is_whitespace())?.0;
    let (start, sentence) = head.split_sentence_bound_indices().find(|(i, s)| last >= *i && last < i + s.len())?;
    let trimmed_start = start + (sentence.len() - sentence.trim_start().len());
    let trimmed_end = start + sentence.trim_end().len();
    let chars = |bytes: usize| text[..bytes].chars().count();
    (trimmed_end > trimmed_start).then(|| (chars(trimmed_start), chars(trimmed_end)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_count_characters_so_emoji_dont_shift_them() {
        let text = "Rhyan 👋🏽 sent the KPI report.";
        // AT-SPI counts characters: the waving hand and its skin tone are two, "sent" starts at 9.
        let range = char_range_to_bytes(text, 9, 13).unwrap();
        assert_eq!(&text[range], "sent");
        assert_eq!(char_range_to_bytes(text, 0, text.chars().count()).unwrap(), 0..text.len());
        assert_eq!(char_range_to_bytes(text, 9, 400), None);
        // UTF-16 would have put "sent" at 11: the two emoji are four units.
        assert_eq!(text.encode_utf16().count() - text.chars().count(), 2);
    }

    #[test]
    fn the_sentence_before_the_caret_is_found_past_emoji() {
        let text = "Done 🎉. Brynlee sent teh OKR notes, e.g. the draft.  ";
        let (start, end) = sentence_before(text, text.chars().count()).unwrap();
        let range = char_range_to_bytes(text, start, end).unwrap();
        assert_eq!(&text[range], "Brynlee sent teh OKR notes, e.g. the draft.");
        assert_eq!(start, 8, "a character offset, counting 🎉 as one");
    }

    #[test]
    fn a_caret_inside_a_sentence_takes_the_sentence_up_to_it() {
        let text = "First one. Kyndal is writ";
        let (start, end) = sentence_before(text, text.chars().count()).unwrap();
        assert_eq!((start, end), (11, 25));
        assert_eq!(sentence_before("   ", 3), None);
        assert_eq!(sentence_before("", 0), None);
    }

    #[test]
    fn a_field_with_embedded_objects_isnt_fully_readable() {
        let field = Field {
            id: FieldId { bus: ":1.42".into(), path: "/org/a11y/atspi/accessible/7".into() },
            editable: true,
            password: false,
            start: 0,
            end: 2,
            selected: "\u{FFFC}\u{FFFC}".into(),
            caret: 2,
            bounds: None,
            window_bounds: None,
        };
        assert!(field.has_selection());
        assert!(field.partial());
    }
}
