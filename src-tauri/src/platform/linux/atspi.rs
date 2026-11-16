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
use atspi::proxy::hyperlink::HyperlinkProxy;
use atspi::proxy::hypertext::HypertextProxy;
use atspi::proxy::text::TextProxy;
use atspi::{CoordType, Role, State};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use supereditr_core::rich::{self, Object, Piece};
use zbus::proxy::CacheProperties;

/// The longest one AT-SPI call may take. Apps usually answer in a few milliseconds, but on a busy
/// machine an app that's fine can take a few hundred, and a call that times out makes the review
/// copy-only; a frozen app is still given up on quickly (see `capture::FIELD_LIMIT`).
const CALL_LIMIT: Duration = Duration::from_millis(1000);
/// The longest a search of an app's tree for its focused field may take, and how many objects it
/// looks at, for an app SuperEditr hasn't seen focus move in.
const SEARCH_LIMIT: Duration = Duration::from_millis(1200);
const SEARCH_NODES: usize = 4000;
/// The longest selection SuperEditr reads, in characters.
const MAX_CHARS: i32 = 50_000;
/// How far back from the caret the sentence before it is looked for.
const SENTENCE_REACH: i32 = 4000;
/// The most embedded objects (a web editor's paragraphs, links and code) read in one field, how
/// deep they may nest, and how long reading them may take.
const MAX_OBJECTS: usize = 2000;
const MAX_DEPTH: usize = 12;
const EMBEDDED_LIMIT: Duration = Duration::from_millis(1500);
/// The character that stands for an embedded object in its parent's text.
const OBJECT: char = '\u{FFFC}';

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
    /// Character ranges of `selected` the app shows as code (`<code>` in a web page).
    pub code: Vec<(usize, usize)>,
    /// What is formatted how in `selected`, when the field holds embedded objects: headings, lists,
    /// code blocks, bold, links.
    pub rich: Option<rich::Node>,
    /// The field's text holds embedded objects, as a web editor's paragraphs are, and `selected`
    /// is their text put together; `start` and `end` count each object as one character.
    pub embedded: bool,
}

impl Field {
    pub fn has_selection(&self) -> bool {
        self.end > self.start && !self.selected.is_empty()
    }

    /// The selection holds embedded objects (U+FFFC) whose text couldn't be read, such as
    /// Obsidian's lines, so its text isn't all there.
    pub fn partial(&self) -> bool {
        self.selected.contains(OBJECT)
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
    /// The field's whole text, with embedded objects' text put together as in [`Field::selected`].
    async fn whole_text(&self, field: &Field) -> Result<String, ReadError>;
    /// Selects the sentence before the caret and reads the field again; `None` when there's no
    /// sentence there.
    async fn select_sentence(&self, field: &Field) -> Result<Option<Field>, ReadError>;
}

/// Runs one AT-SPI call within [`CALL_LIMIT`]. A slow call is logged with where it was made, since
/// a slow app and a slow bus look the same from here.
#[track_caller]
fn limit<T, E: Into<ReadError>>(
    call: impl Future<Output = Result<T, E>>,
) -> impl Future<Output = Result<T, ReadError>> {
    let caller = std::panic::Location::caller();
    async move {
        let started = Instant::now();
        let result = tokio::time::timeout(CALL_LIMIT, call).await;
        if started.elapsed() > Duration::from_millis(250) {
            super::trace(format_args!("AT-SPI call at line {} took {:?}", caller.line(), started.elapsed()));
        }
        match result {
            Ok(result) => result.map_err(Into::into),
            Err(_) => Err(ReadError::Timeout),
        }
    }
}

/// The session's accessibility bus.
#[derive(Default)]
pub struct Atspi {
    connection: tokio::sync::OnceCell<atspi_connection::AccessibilityConnection>,
    /// The object that most recently reported getting focus in each app (by bus name), from
    /// AT-SPI's events.
    last_focus: Mutex<HashMap<String, FieldId>>,
    /// Each app's bus name and process.
    pids: Mutex<HashMap<String, i32>>,
}

impl Atspi {
    async fn connection(&self) -> Result<&atspi_connection::AccessibilityConnection, ReadError> {
        self.connection
            .get_or_try_init(|| async {
                match tokio::time::timeout(Duration::from_secs(2), atspi_connection::AccessibilityConnection::new())
                    .await
                {
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

    async fn hypertext_proxy(&self, id: &FieldId) -> Result<HypertextProxy<'static>, ReadError> {
        let connection = self.connection().await?.connection();
        Ok(HypertextProxy::builder(connection)
            .destination(id.bus.clone())?
            .path(id.path.clone())?
            .cache_properties(CacheProperties::No)
            .build()
            .await?)
    }

    /// The object a hyperlink (one of a parent's embedded objects) stands for.
    async fn linked_object(&self, bus: &str, path: &str) -> Result<FieldId, ReadError> {
        let connection = self.connection().await?.connection();
        let link = HyperlinkProxy::builder(connection)
            .destination(bus.to_string())?
            .path(path.to_string())?
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        let object = limit(link.get_object(0)).await?;
        Ok(FieldId { bus: object.name_as_str().unwrap_or(bus).to_string(), path: object.path_as_str().to_string() })
    }

    /// The text between `start` and `end` in an object, as pieces: runs of its own text, and the
    /// embedded objects in it with their text, read the same way. An object's text counts only
    /// what's selected in it, when it reports a selection, so a selection that starts or ends inside
    /// a paragraph takes only that part of it.
    fn pieces<'a>(
        &'a self,
        id: FieldId,
        start: i32,
        end: i32,
        depth: usize,
        seen: &'a mut usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Piece>, ReadError>> + Send + 'a>> {
        Box::pin(async move {
            if depth > MAX_DEPTH {
                return Err(ReadError::Failed("embedded objects nest too deep".into()));
            }
            let raw = limit(self.text_proxy(&id).await?.get_text(start, end)).await?;
            if !raw.contains(OBJECT) {
                return Ok(vec![Piece::Text(raw)]);
            }
            let hypertext = self.hypertext_proxy(&id).await?;
            let mut pieces = Vec::new();
            let mut run = String::new();
            for (i, c) in raw.chars().enumerate() {
                if c != OBJECT {
                    run.push(c);
                    continue;
                }
                if !run.is_empty() {
                    pieces.push(Piece::Text(std::mem::take(&mut run)));
                }
                *seen += 1;
                if *seen > MAX_OBJECTS {
                    return Err(ReadError::TooLong);
                }
                let index = limit(hypertext.get_link_index(start + i as i32)).await?;
                if index < 0 {
                    return Err(ReadError::Failed("an embedded object has no link".into()));
                }
                let link = limit(hypertext.get_link(index)).await?;
                let child = self.linked_object(link.name_as_str().unwrap_or(&id.bus), link.path_as_str()).await?;
                let attributes = limit(self.accessible(&child.bus, &child.path).await?.get_attributes()).await?;
                let tag = attributes.get("tag").cloned().unwrap_or_default();
                let display = attributes.get("display").map(String::as_str).unwrap_or("inline");
                let block = !display.starts_with("inline");
                let href = match tag.as_str() {
                    "a" => self.link_uri(link.name_as_str().unwrap_or(&id.bus), link.path_as_str()).await,
                    _ => None,
                };
                let text = self.text_proxy(&child).await?;
                let count = limit(text.character_count()).await?;
                let (from, to) = match limit(text.get_n_selections()).await {
                    Ok(n) if n > 0 => limit(text.get_selection(0)).await?,
                    _ => (0, count),
                };
                let (from, to) = (from.min(to).clamp(0, count), to.max(from).clamp(0, count));
                // A list item's text starts with its bullet or number.
                let marker = if display == "list-item" && from == 0 { self.marker_of(&child).await } else { None };
                let inner = self.pieces(child, from, to, depth + 1, seen).await?;
                pieces.push(Piece::Object(Object { tag, block, href, marker, pieces: inner }));
            }
            if !run.is_empty() {
                pieces.push(Piece::Text(run));
            }
            Ok(pieces)
        })
    }

    /// Where a hyperlink goes, when it says.
    async fn link_uri(&self, bus: &str, path: &str) -> Option<String> {
        let connection = self.connection().await.ok()?.connection();
        let link = HyperlinkProxy::builder(connection)
            .destination(bus.to_string())
            .ok()?
            .path(path.to_string())
            .ok()?
            .cache_properties(CacheProperties::No)
            .build()
            .await
            .ok()?;
        limit(link.get_uri(0)).await.ok().filter(|uri| !uri.is_empty())
    }

    /// A list item's bullet or number, as its text starts with it ("• ", "1. "), when the app
    /// reports one: its first child, which a browser gives the tag `::marker`.
    async fn marker_of(&self, item: &FieldId) -> Option<String> {
        let children = limit(self.accessible(&item.bus, &item.path).await.ok()?.get_children()).await.ok()?;
        let first = children.first()?;
        let id = FieldId {
            bus: first.name_as_str().unwrap_or(&item.bus).to_string(),
            path: first.path_as_str().to_string(),
        };
        let attributes = limit(self.accessible(&id.bus, &id.path).await.ok()?.get_attributes()).await.ok()?;
        if attributes.get("tag").is_none_or(|tag| tag != "::marker") {
            return None;
        }
        let text = self.text_proxy(&id).await.ok()?;
        let count = limit(text.character_count()).await.ok()?;
        limit(text.get_text(0, count)).await.ok()
    }

    /// The text between `start` and `end` with its embedded objects' text put together, and what
    /// is formatted how in it.
    async fn embedded_text(&self, id: &FieldId, start: i32, end: i32) -> Result<rich::Rich, ReadError> {
        let mut seen = 0;
        match tokio::time::timeout(EMBEDDED_LIMIT, self.pieces(id.clone(), start, end, 0, &mut seen)).await {
            Ok(pieces) => Ok(rich::assemble(&pieces?)),
            Err(_) => Err(ReadError::Timeout),
        }
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

    /// The processes of every app registered on the accessibility bus.
    pub async fn registered_pids(&self) -> Result<Vec<i32>, ReadError> {
        let root = self.accessible("org.a11y.atspi.Registry", "/org/a11y/atspi/accessible/root").await?;
        let mut pids = Vec::new();
        for app in limit(root.get_children()).await? {
            if let Some(bus) = app.name_as_str()
                && let Ok(pid) = self.pid_of(bus).await
            {
                pids.push(pid);
            }
        }
        Ok(pids)
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
            code: Vec::new(),
            rich: None,
            embedded: false,
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
        // A web editor's paragraphs, links and code are embedded objects: read their text. If they
        // can't be read, the field stays partial, and a shortcut falls back to a copy-only read.
        let (selected, code, rich, embedded) = match selected.contains(OBJECT) {
            true => match self.embedded_text(id, start, end).await {
                Ok(read) => (read.text, read.root.code(), Some(read.root), true),
                Err(_) => (selected, Vec::new(), None, false),
            },
            false => (selected, Vec::new(), None, false),
        };
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
        Ok(Some(Field { start, end, selected, caret, bounds, window_bounds, code, rich, embedded, ..blank }))
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

    async fn whole_text(&self, field: &Field) -> Result<String, ReadError> {
        let count = limit(self.text_proxy(&field.id).await?.character_count()).await?;
        if field.embedded {
            return Ok(self.embedded_text(&field.id, 0, count).await?.text);
        }
        self.text(&field.id, 0, count).await
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
            code: Vec::new(),
            rich: None,
            embedded: false,
        };
        assert!(field.has_selection());
        assert!(field.partial());
    }
}
