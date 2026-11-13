//! The clipboard as a transaction: the user's clipboard is put aside, the suggestion goes on it for
//! one paste, and the user's clipboard comes back.
//!
//! - **Snapshot.** Every format is copied by a short-lived child process, SuperEditr's own program
//!   started with [`SNAPSHOT_COMMAND`], which writes what it reads to its output. Reading a format
//!   another app renders on demand blocks for as long as that app takes (up to 30 s), with the
//!   clipboard open, so no other app can use it meanwhile. At the deadline the child is killed, and
//!   Windows closes a dead process's clipboard: other apps can use it at once. Formats Windows makes
//!   again from an earlier one (`CF_TEXT` from `CF_UNICODETEXT`, `CF_BITMAP` from `CF_DIB`...) are
//!   skipped, and a bitmap is read as `CF_DIB`'s bytes. One format may hold 64 MB and the snapshot
//!   256 MB, checked before anything is copied. A format the owning app answers with no data and no
//!   error is absent for every app and doesn't count.
//! - **Incomplete** means the clipboard stayed open in another app, the deadline passed, a format
//!   was over a limit, or a format can't be copied. Nothing is pasted then, and the clipboard isn't
//!   touched; the panel offers to replace anyway, without putting the clipboard aside.
//!   capture.log names the format being waited on and the app that owns the clipboard.
//! - **Write** only while the sequence number is still the snapshot's, checked with the clipboard
//!   open, with `ExcludeClipboardContentFromMonitorProcessing` so clipboard history skips it.
//!   Emptying the clipboard asks its owner to let go (`WM_DESTROYCLIPBOARD`), and Windows waits up
//!   to 5 s for an owner that doesn't answer before going ahead.
//! - **Restore** only while the sequence number is still SuperEditr's: anything newer is the
//!   user's.
//! - **A late copy.** A guarded copy that gave up on an app may still see the app copy afterwards,
//!   when it gets to the Ctrl+C. For a few seconds the put-aside clipboard is kept, and a change the
//!   app itself made is undone, once per Ctrl+C sent. A change by any other program, or any change
//!   after the user pressed a copy key themselves, is the user's: it stays, and the watch ends. The
//!   next transaction settles the watch first.
//!
//! [`Raw`] is the clipboard itself, so the transaction can be tested against a fake.

use std::io;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const CF_TEXT: u32 = 1;
pub const CF_BITMAP: u32 = 2;
pub const CF_METAFILEPICT: u32 = 3;
pub const CF_OEMTEXT: u32 = 7;
pub const CF_DIB: u32 = 8;
pub const CF_PALETTE: u32 = 9;
pub const CF_UNICODETEXT: u32 = 13;
pub const CF_ENHMETAFILE: u32 = 14;
pub const CF_DIBV5: u32 = 17;
pub const CF_OWNERDISPLAY: u32 = 0x80;
pub const CF_DSPBITMAP: u32 = 0x82;
pub const CF_DSPMETAFILEPICT: u32 = 0x83;
pub const CF_DSPENHMETAFILE: u32 = 0x8E;

/// The argument that makes SuperEditr's program read the clipboard for a snapshot and exit.
pub const SNAPSHOT_COMMAND: &str = "clipboard-snapshot";

/// What one format held, as bytes: a snapshot holds no handles, so it can come from another process.
#[derive(Debug, PartialEq)]
pub enum Data {
    /// A global memory block's bytes.
    Bytes(Vec<u8>),
    /// An enhanced metafile's bits.
    Emf(Vec<u8>),
    /// `CF_METAFILEPICT`: its header and the metafile's bits, since the block holds a handle.
    MetafilePict { mm: i32, x: i32, y: i32, bits: Vec<u8> },
}

impl Data {
    pub fn size(&self) -> usize {
        match self {
            Data::Bytes(b) | Data::Emf(b) | Data::MetafilePict { bits: b, .. } => b.len(),
        }
    }
}

/// Reading one format.
#[derive(Debug)]
pub enum Read {
    Data(Data),
    /// The owner gave no data and no error: absent for every app.
    Absent,
    /// Bigger than the limit it was read with.
    TooLarge(usize),
    Failed(String),
}

/// What a snapshot's reader reports, in order: the clipboard opened, then each format as it's read,
/// then how it ended.
#[derive(Debug, PartialEq)]
pub enum Event {
    /// The clipboard is open; its sequence number.
    Opened(u32),
    /// Reading this format now: the one a stall is waiting on.
    Reading(u32),
    Format(u32, Data),
    Done(Result<(), Incomplete>),
}

/// Stops a snapshot's reader. Once it returns, the reader's clipboard is closed.
pub type Stop = Box<dyn FnOnce() + Send>;

/// The system clipboard. `open`, `formats`, `read` and `close` run on one thread, in that order.
pub trait Raw: Send + Sync + 'static {
    /// One attempt to open the clipboard.
    fn open(&self) -> bool;
    fn close(&self);
    fn sequence(&self) -> u32;
    /// Formats in `EnumClipboardFormats` order. The clipboard is open.
    fn formats(&self) -> Vec<u32>;
    /// Copies a format, refusing one bigger than `limit` bytes. The clipboard is open.
    fn read(&self, format: u32, limit: usize) -> Read;
    /// Empties the clipboard and makes SuperEditr its owner. The clipboard is open.
    fn empty(&self) -> bool;
    /// Hands a copy of `data` to the clipboard. The clipboard is open.
    fn set(&self, format: u32, data: &Data) -> bool;
    /// The registered `ExcludeClipboardContentFromMonitorProcessing` format.
    fn exclude_format(&self) -> u32;
    /// The registered `HTML Format` (CF_HTML).
    fn html_format(&self) -> u32;
    /// The program holding the clipboard open, if one is.
    fn holder(&self) -> Option<String>;
    /// The program that put the clipboard's contents there, which renders formats on demand, by
    /// its readable name ([`super::apps::display_name`]).
    fn owner(&self) -> Option<String>;
    /// A registered format's name, for capture.log: "HTML Format", or the number otherwise.
    fn format_name(&self, format: u32) -> String;
    /// Starts [`read_all`] where a stuck read can be stopped, sending its events to `events`. The
    /// returned [`Stop`] must close the clipboard at once, whatever read the reader is blocked in.
    fn start_reader(self: Arc<Self>, limits: Limits, events: Sender<Event>) -> Result<Stop, String>;
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub per_format: usize,
    pub total: usize,
    /// How long another app may hold the clipboard open.
    pub open_wait: Duration,
    /// How long starting the reader and reading every format may take once it's open.
    pub deadline: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            per_format: 64 << 20,
            total: 256 << 20,
            open_wait: Duration::from_millis(500),
            deadline: Duration::from_millis(if cfg!(test) { 300 } else { 1000 }),
        }
    }
}

/// The user's clipboard, put aside.
#[derive(Debug)]
pub struct Snapshot {
    pub sequence: u32,
    pub formats: Vec<(u32, Data)>,
}

/// Why the clipboard couldn't be put aside. The format numbers are for capture.log.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incomplete {
    /// Another app kept it open. Holds that app's name when known.
    Held(Option<String>),
    /// An app rendering a format on demand didn't answer in time.
    TimedOut,
    TooLarge {
        format: u32,
        bytes: usize,
    },
    NotCopyable(u32),
    Failed(String),
}

impl std::fmt::Display for Incomplete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Incomplete::Held(Some(app)) => write!(f, "{app} is holding the clipboard"),
            Incomplete::Held(None) => write!(f, "Another app is holding the clipboard"),
            Incomplete::TimedOut => {
                write!(f, "The app that put the clipboard’s contents there isn’t answering")
            }
            Incomplete::TooLarge { bytes, .. } => {
                write!(f, "What’s on your clipboard is too big to put aside ({} MB)", bytes >> 20)
            }
            Incomplete::NotCopyable(_) => write!(f, "What’s on your clipboard can’t be put aside"),
            Incomplete::Failed(why) => write!(f, "Your clipboard couldn’t be put aside ({why})"),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum WriteError {
    /// Something else wrote the clipboard since the snapshot: the user copied.
    Changed,
    Held,
    Failed,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Restore {
    Restored,
    /// Something newer than SuperEditr's text is on the clipboard, and it stays.
    Newer,
    Held,
    Failed,
}

/// Formats Windows makes from another of the same group, so a snapshot keeps only the first.
fn synth_group(format: u32) -> u32 {
    match format {
        CF_TEXT | CF_OEMTEXT | CF_UNICODETEXT => 1,
        CF_BITMAP | CF_DIB | CF_DIBV5 => 2,
        CF_METAFILEPICT | CF_ENHMETAFILE => 3,
        _ => 0,
    }
}

/// Formats nothing can copy generically. `CF_DSPBITMAP` holds a bitmap handle that, unlike
/// `CF_BITMAP`'s, Windows offers as no other format.
fn not_copyable(format: u32) -> bool {
    matches!(format, CF_PALETTE | CF_OWNERDISPLAY | CF_DSPBITMAP | 0x300..=0x3FF)
}

/// A standard format's name, for capture.log.
pub fn standard_name(format: u32) -> Option<&'static str> {
    Some(match format {
        1 => "CF_TEXT",
        2 => "CF_BITMAP",
        3 => "CF_METAFILEPICT",
        4 => "CF_SYLK",
        5 => "CF_DIF",
        6 => "CF_TIFF",
        7 => "CF_OEMTEXT",
        8 => "CF_DIB",
        9 => "CF_PALETTE",
        10 => "CF_PENDATA",
        11 => "CF_RIFF",
        12 => "CF_WAVE",
        13 => "CF_UNICODETEXT",
        14 => "CF_ENHMETAFILE",
        15 => "CF_HDROP",
        16 => "CF_LOCALE",
        17 => "CF_DIBV5",
        0x80 => "CF_OWNERDISPLAY",
        0x81 => "CF_DSPTEXT",
        0x82 => "CF_DSPBITMAP",
        0x83 => "CF_DSPMETAFILEPICT",
        0x8E => "CF_DSPENHMETAFILE",
        0x200..=0x2FF => "a private format",
        0x300..=0x3FF => "a GDI object format",
        _ => return None,
    })
}

/// A format as capture.log names it: "CF_UNICODETEXT (13)", "HTML Format (49361)".
pub fn label(raw: &dyn Raw, format: u32) -> String {
    let name = standard_name(format).map_or_else(|| raw.format_name(format), str::to_string);
    format!("{name} ({format})")
}

/// Opens the clipboard, trying every 5 ms within `wait`.
fn open_within(raw: &dyn Raw, wait: Duration) -> bool {
    let started = Instant::now();
    loop {
        if raw.open() {
            return true;
        }
        if started.elapsed() >= wait {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Copies every format, reporting each to `emit` as it goes. Runs in the snapshot's reader.
pub fn read_all(raw: &dyn Raw, limits: &Limits, emit: &mut dyn FnMut(Event)) {
    if !open_within(raw, limits.open_wait) {
        return emit(Event::Done(Err(Incomplete::Held(raw.holder()))));
    }
    emit(Event::Opened(raw.sequence()));
    let ids = raw.formats();
    let mut used = 0usize;
    let mut result = Ok(());
    for (i, &format) in ids.iter().enumerate() {
        let group = synth_group(format);
        if group != 0 && ids[..i].iter().any(|&earlier| synth_group(earlier) == group) {
            continue;
        }
        if not_copyable(format) {
            result = Err(Incomplete::NotCopyable(format));
            break;
        }
        // A bitmap handle can't leave the process: its pixels are read as a DIB, from which Windows
        // makes CF_BITMAP again.
        let format = if format == CF_BITMAP { CF_DIB } else { format };
        let limit = limits.per_format.min(limits.total.saturating_sub(used));
        emit(Event::Reading(format));
        match raw.read(format, limit) {
            Read::Data(data) => {
                used += data.size();
                emit(Event::Format(format, data));
            }
            Read::Absent => {}
            Read::TooLarge(bytes) => {
                result = Err(Incomplete::TooLarge { format, bytes });
                break;
            }
            Read::Failed(why) => {
                result = Err(Incomplete::Failed(why));
                break;
            }
        }
    }
    raw.close();
    emit(Event::Done(result));
}

/// How long a late copy is watched for after a guarded copy gave up. A Chromium browser busy
/// building its accessibility tree took its Ctrl+C after the copy had been given up on.
const LATE_WAIT: Duration = Duration::from_millis(if cfg!(test) { 300 } else { 5000 });
const LATE_POLL: Duration = Duration::from_millis(if cfg!(test) { 2 } else { 10 });

/// A guarded copy's put-aside clipboard, kept while the app may still copy.
struct Late {
    id: u64,
    snapshot: Snapshot,
    /// The app the Ctrl+C went to, as [`Raw::owner`] names it.
    app: String,
    /// The sequence number last seen: the snapshot's, or SuperEditr's after it undid a late copy.
    seen: u32,
    /// Copies still to come: one per Ctrl+C sent, less those seen.
    copies: usize,
    /// Whether the user has pressed a copy key themselves since SuperEditr's first Ctrl+C. After
    /// that, a copy from the app may be theirs, so the watch ends and the clipboard stays as it is.
    user_acted: super::capture::InputWatch,
}

pub struct Clipboard<R: Raw> {
    raw: Arc<R>,
    /// The format the latest snapshot was reading when it ended, or 0.
    waiting: AtomicU32,
    late: Arc<Mutex<Option<Late>>>,
    watches: AtomicU64,
}

impl<R: Raw> Clipboard<R> {
    pub fn new(raw: R) -> Self {
        Clipboard { raw: Arc::new(raw), waiting: AtomicU32::new(0), late: Arc::default(), watches: AtomicU64::new(0) }
    }

    #[cfg(test)]
    pub fn raw(&self) -> &R {
        &self.raw
    }

    #[cfg(test)]
    pub fn raw_arc(&self) -> Arc<R> {
        self.raw.clone()
    }

    /// After a guarded copy: keeps `snapshot` for a few seconds, on a thread of its own, and puts it
    /// back if `app` copies after all, up to `copies` times, as long as the user hasn't pressed a
    /// copy key themselves (`user_acted`). Anything another program puts on the clipboard, and
    /// anything after the user's own copy key, is the user's: it stays, and the watch ends.
    pub fn watch_late_copies(
        &self,
        snapshot: Snapshot,
        app: String,
        copies: usize,
        user_acted: super::capture::InputWatch,
    ) {
        if copies == 0 {
            return;
        }
        let id = self.watches.fetch_add(1, Ordering::SeqCst) + 1;
        let seen = self.raw.sequence();
        *self.late.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(Late { id, snapshot, app, seen, copies, user_acted });
        let (raw, late) = (self.raw.clone(), self.late.clone());
        let watching = std::thread::Builder::new().name("late copy".into()).spawn(move || {
            let until = Instant::now() + LATE_WAIT;
            loop {
                std::thread::sleep(LATE_POLL);
                let mut slot = late.lock().unwrap_or_else(|p| p.into_inner());
                // Settled by a later transaction, or replaced by a newer watch.
                let Some(watch) = slot.as_mut().filter(|w| w.id == id) else { return };
                if !undo_late_copy(&*raw, watch) || Instant::now() >= until {
                    *slot = None;
                    return;
                }
            }
        });
        if watching.is_err() {
            // No thread: look once now, then give the watch up.
            self.settle_late();
        }
    }

    /// Before any other transaction: a late copy that already came is undone, and the watch ends.
    fn settle_late(&self) {
        let watch = self.late.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(mut watch) = watch {
            undo_late_copy(&*self.raw, &mut watch);
        }
    }

    pub fn sequence(&self) -> u32 {
        self.raw.sequence()
    }

    /// What a snapshot past its deadline was waiting for: "format HTML Format (49361) from Notes".
    pub fn stuck(&self) -> String {
        let format = self.waiting.load(Ordering::Acquire);
        let owner = self.raw.owner().unwrap_or_else(|| "an unknown app".into());
        if format == 0 {
            return format!("the clipboard of {owner}");
        }
        format!("format {} from {owner}", label(&*self.raw, format))
    }

    /// Puts the user's clipboard aside, or says why it can't. Past the deadline the reader is
    /// stopped, which closes the clipboard, so a stalled app never keeps it from anyone else.
    pub fn snapshot(&self, limits: &Limits) -> Result<Snapshot, Incomplete> {
        self.settle_late();
        let started = Instant::now();
        let (tx, rx) = channel();
        self.waiting.store(0, Ordering::Release);
        let stop = self.raw.clone().start_reader(*limits, tx).map_err(Incomplete::Failed)?;
        let until = started + limits.open_wait + limits.deadline;
        let mut sequence = None;
        let mut formats = Vec::new();
        let result = loop {
            match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(Event::Opened(at)) => sequence = Some(at),
                Ok(Event::Reading(format)) => self.waiting.store(format, Ordering::Release),
                Ok(Event::Format(format, data)) => {
                    self.waiting.store(0, Ordering::Release);
                    formats.push((format, data));
                }
                Ok(Event::Done(Ok(()))) => match sequence {
                    Some(sequence) => break Ok(Snapshot { sequence, formats }),
                    None => break Err(Incomplete::Failed("no sequence number".into())),
                },
                Ok(Event::Done(Err(why))) => break Err(why),
                Err(RecvTimeoutError::Timeout) => break Err(Incomplete::TimedOut),
                Err(RecvTimeoutError::Disconnected) => break Err(Incomplete::Failed("the reader stopped".into())),
            }
        };
        stop();
        match &result {
            Ok(snapshot) => {
                let bytes: usize = snapshot.formats.iter().map(|(_, data)| data.size()).sum();
                super::trace(format_args!(
                    "clipboard: put aside {} formats, {} KB, in {:?}",
                    snapshot.formats.len(),
                    bytes.div_ceil(1024),
                    started.elapsed()
                ));
            }
            Err(Incomplete::TimedOut) => {
                // Opened and closed again at once: capture.log shows whether other apps can use it.
                let free = self.raw.open();
                if free {
                    self.raw.close();
                }
                super::trace(format_args!(
                    "clipboard: timed out after {:?} waiting on {}; the reader was stopped, and the clipboard {}",
                    started.elapsed(),
                    self.stuck(),
                    if free { "is free".to_string() } else { format!("is held by {:?}", self.raw.holder()) }
                ));
            }
            Err(why) => super::trace(format_args!("clipboard: not put aside ({why:?})")),
        }
        result
    }

    /// Puts `text` on the clipboard, with `html` as its HTML version when given, marked to stay out
    /// of clipboard history, if the clipboard is still `expected`. Returns the new sequence number,
    /// which is SuperEditr's.
    pub fn write_text(&self, expected: u32, text: &str, html: Option<&str>) -> Result<u32, WriteError> {
        self.write(Some(expected), text, html)
    }

    /// As [`Clipboard::write_text`], whatever is there: the user chose to replace without their
    /// clipboard put aside.
    pub fn write_text_anyway(&self, text: &str, html: Option<&str>) -> Result<u32, WriteError> {
        self.write(None, text, html)
    }

    fn write(&self, expected: Option<u32>, text: &str, html: Option<&str>) -> Result<u32, WriteError> {
        self.settle_late();
        if !open_within(&*self.raw, Limits::default().open_wait) {
            return Err(WriteError::Held);
        }
        if expected.is_some_and(|expected| self.raw.sequence() != expected) {
            self.raw.close();
            return Err(WriteError::Changed);
        }
        let written = self.raw.empty()
            && self.raw.set(CF_UNICODETEXT, &Data::Bytes(utf16z(text)))
            && html.is_none_or(|html| self.raw.set(self.raw.html_format(), &Data::Bytes(cf_html(html))))
            && self.raw.set(self.raw.exclude_format(), &Data::Bytes(vec![0, 0, 0, 0]));
        self.raw.close();
        if written { Ok(self.raw.sequence()) } else { Err(WriteError::Failed) }
    }

    /// The program holding the clipboard open, if one is.
    pub fn holder(&self) -> Option<String> {
        self.raw.holder()
    }

    /// Puts `snapshot` back if the clipboard still holds what SuperEditr wrote (`ours`).
    pub fn restore(&self, snapshot: &Snapshot, ours: u32) -> Restore {
        restore(&*self.raw, snapshot, ours)
    }

    /// The clipboard's text, for a guarded copy.
    pub fn text(&self) -> Option<String> {
        if !open_within(&*self.raw, Limits::default().open_wait) {
            return None;
        }
        let read = self.raw.read(CF_UNICODETEXT, 64 << 20);
        self.raw.close();
        match read {
            Read::Data(Data::Bytes(bytes)) => Some(from_utf16z(&bytes)),
            _ => None,
        }
    }

    /// Puts `text` on the clipboard for the user to paste: an ordinary copy.
    pub fn copy(&self, text: &str) -> bool {
        self.settle_late();
        if !open_within(&*self.raw, Limits::default().open_wait) {
            return false;
        }
        let written = self.raw.empty() && self.raw.set(CF_UNICODETEXT, &Data::Bytes(utf16z(text)));
        self.raw.close();
        written
    }
}

/// Puts `snapshot` back if the clipboard's sequence number is still `ours`.
fn restore(raw: &dyn Raw, snapshot: &Snapshot, ours: u32) -> Restore {
    if !open_within(raw, Limits::default().open_wait) {
        return Restore::Held;
    }
    if raw.sequence() != ours {
        raw.close();
        return Restore::Newer;
    }
    let mut ok = raw.empty();
    for (format, data) in &snapshot.formats {
        ok &= raw.set(*format, data);
    }
    raw.close();
    if ok { Restore::Restored } else { Restore::Failed }
}

/// One look at a late-copy watch: undoes a copy the app made since the last look. False when the
/// watch is over: every Ctrl+C's copy was undone, another program wrote the clipboard, or it
/// couldn't be put back.
fn undo_late_copy(raw: &dyn Raw, watch: &mut Late) -> bool {
    // A copy the user made themselves must never be lost; one the app makes late, of their own
    // selection, is the lesser harm.
    if (watch.user_acted)() {
        super::trace(format_args!("clipboard: the user copied after a guarded copy in {}; theirs stays", watch.app));
        return false;
    }
    let now = raw.sequence();
    if now == watch.seen {
        return true;
    }
    let owner = raw.owner();
    if owner.as_deref() != Some(watch.app.as_str()) {
        super::trace(format_args!(
            "clipboard: {} wrote it after a guarded copy in {}; theirs stays",
            owner.as_deref().unwrap_or("another app"),
            watch.app
        ));
        return false;
    }
    match restore(raw, &watch.snapshot, now) {
        Restore::Restored => {
            watch.seen = raw.sequence();
            watch.copies -= 1;
            super::trace(format_args!("clipboard: {} copied late; the clipboard was put back", watch.app));
            watch.copies > 0
        }
        // It changed again, or is still being written: look again.
        Restore::Newer | Restore::Held => true,
        Restore::Failed => {
            super::trace(format_args!("clipboard: {} copied late, and the clipboard couldn't be put back", watch.app));
            false
        }
    }
}

/// `text` as HTML for pasting, or None when HTML can't hold it exactly: HTML collapses runs of
/// spaces and drops them at the ends of a line. Each line is a paragraph, as a plain-text paste
/// into a rich editor makes it, an empty one included. A rich editor pastes HTML with its own
/// formatting only, where a plain-text paste takes the formatting at the selection's start, such as
/// inline code.
pub fn html_paragraphs(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)).collect();
    let exact = |line: &&str| {
        !line.starts_with(' ')
            && !line.ends_with(' ')
            && !line.contains("  ")
            && !line.chars().any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    };
    if !lines.iter().all(exact) {
        return None;
    }
    let paragraphs = lines.len() > 1;
    let mut html = String::new();
    for line in lines {
        html.push_str(if paragraphs { "<p>" } else { "" });
        for c in line.chars() {
            match c {
                '&' => html.push_str("&amp;"),
                '<' => html.push_str("&lt;"),
                '>' => html.push_str("&gt;"),
                '"' => html.push_str("&quot;"),
                c => html.push(c),
            }
        }
        if paragraphs {
            html.push_str(if line.is_empty() { "<br></p>" } else { "</p>" });
        }
    }
    Some(html)
}

/// `fragment` in the clipboard's HTML Format: a header of byte offsets, then the document, UTF-8 and
/// ending in a NUL.
pub fn cf_html(fragment: &str) -> Vec<u8> {
    const BEFORE: &str = "<html><body><!--StartFragment-->";
    const AFTER: &str = "<!--EndFragment--></body></html>";
    let header = |start: usize, end: usize, from: usize, to: usize| {
        format!(
            "Version:0.9\r\nStartHTML:{start:010}\r\nEndHTML:{end:010}\r\nStartFragment:{from:010}\r\nEndFragment:{to:010}\r\n"
        )
    };
    let start = header(0, 0, 0, 0).len();
    let from = start + BEFORE.len();
    let to = from + fragment.len();
    let end = to + AFTER.len();
    let mut out = header(start, end, from, to).into_bytes();
    out.extend_from_slice(BEFORE.as_bytes());
    out.extend_from_slice(fragment.as_bytes());
    out.extend_from_slice(AFTER.as_bytes());
    out.push(0);
    out
}

pub fn utf16z(text: &str) -> Vec<u8> {
    text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

fn from_utf16z(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// How a snapshot's events travel from the child process to SuperEditr: a tag byte, then
/// little-endian numbers and length-prefixed bytes.
pub mod wire {
    use super::{Data, Event, Incomplete};
    use std::io::{self, Read, Write};

    /// No single format is bigger than the snapshot's total limit.
    const MOST: u64 = 1 << 30;

    fn u32_(out: &mut dyn Write, n: u32) -> io::Result<()> {
        out.write_all(&n.to_le_bytes())
    }

    fn bytes(out: &mut dyn Write, b: &[u8]) -> io::Result<()> {
        out.write_all(&(b.len() as u64).to_le_bytes())?;
        out.write_all(b)
    }

    pub fn write(out: &mut dyn Write, event: &Event) -> io::Result<()> {
        match event {
            Event::Opened(sequence) => {
                out.write_all(b"O")?;
                u32_(out, *sequence)
            }
            Event::Reading(format) => {
                out.write_all(b"R")?;
                u32_(out, *format)
            }
            Event::Format(format, data) => {
                out.write_all(b"F")?;
                u32_(out, *format)?;
                match data {
                    Data::Bytes(b) => {
                        out.write_all(&[0])?;
                        bytes(out, b)
                    }
                    Data::Emf(b) => {
                        out.write_all(&[1])?;
                        bytes(out, b)
                    }
                    Data::MetafilePict { mm, x, y, bits } => {
                        out.write_all(&[2])?;
                        for n in [mm, x, y] {
                            out.write_all(&n.to_le_bytes())?;
                        }
                        bytes(out, bits)
                    }
                }
            }
            Event::Done(Ok(())) => out.write_all(b"K"),
            Event::Done(Err(why)) => {
                out.write_all(b"X")?;
                match why {
                    Incomplete::Held(app) => {
                        out.write_all(&[0])?;
                        bytes(out, app.as_deref().unwrap_or("").as_bytes())
                    }
                    Incomplete::TimedOut => out.write_all(&[1]),
                    Incomplete::TooLarge { format, bytes: size } => {
                        out.write_all(&[2])?;
                        u32_(out, *format)?;
                        out.write_all(&(*size as u64).to_le_bytes())
                    }
                    Incomplete::NotCopyable(format) => {
                        out.write_all(&[3])?;
                        u32_(out, *format)
                    }
                    Incomplete::Failed(text) => {
                        out.write_all(&[4])?;
                        bytes(out, text.as_bytes())
                    }
                }
            }
        }
    }

    fn take<const N: usize>(input: &mut dyn Read) -> io::Result<[u8; N]> {
        let mut b = [0u8; N];
        input.read_exact(&mut b)?;
        Ok(b)
    }

    fn read_u32(input: &mut dyn Read) -> io::Result<u32> {
        Ok(u32::from_le_bytes(take(input)?))
    }

    fn read_bytes(input: &mut dyn Read) -> io::Result<Vec<u8>> {
        let n = u64::from_le_bytes(take(input)?);
        if n > MOST {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "too long"));
        }
        let mut b = vec![0u8; n as usize];
        input.read_exact(&mut b)?;
        Ok(b)
    }

    fn bad(what: &str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, what.to_string())
    }

    /// The next event, or None at the end of the stream.
    pub fn read(input: &mut dyn Read) -> io::Result<Option<Event>> {
        let mut tag = [0u8; 1];
        if input.read(&mut tag)? == 0 {
            return Ok(None);
        }
        Ok(Some(match &tag {
            b"O" => Event::Opened(read_u32(input)?),
            b"R" => Event::Reading(read_u32(input)?),
            b"F" => {
                let format = read_u32(input)?;
                let data = match take::<1>(input)?[0] {
                    0 => Data::Bytes(read_bytes(input)?),
                    1 => Data::Emf(read_bytes(input)?),
                    2 => {
                        let [mm, x, y] = [(); 3].map(|_| take::<4>(input).map(i32::from_le_bytes));
                        Data::MetafilePict { mm: mm?, x: x?, y: y?, bits: read_bytes(input)? }
                    }
                    _ => return Err(bad("data kind")),
                };
                Event::Format(format, data)
            }
            b"K" => Event::Done(Ok(())),
            b"X" => Event::Done(Err(match take::<1>(input)?[0] {
                0 => {
                    let app = String::from_utf8_lossy(&read_bytes(input)?).into_owned();
                    Incomplete::Held((!app.is_empty()).then_some(app))
                }
                1 => Incomplete::TimedOut,
                2 => {
                    let format = read_u32(input)?;
                    Incomplete::TooLarge { format, bytes: u64::from_le_bytes(take(input)?) as usize }
                }
                3 => Incomplete::NotCopyable(read_u32(input)?),
                4 => Incomplete::Failed(String::from_utf8_lossy(&read_bytes(input)?).into_owned()),
                _ => return Err(bad("reason")),
            })),
            _ => return Err(bad("tag")),
        }))
    }
}

/// The child process's side of a snapshot: reads the clipboard with the limits in `args` (bytes per
/// format, bytes in all, milliseconds to wait for it to open) and writes the events to `out`.
/// Returns the process's exit code.
pub fn serve_snapshot(raw: &dyn Raw, args: &[String], out: &mut dyn io::Write) -> i32 {
    let number = |i: usize| args.get(i).and_then(|a| a.parse::<u64>().ok());
    let (Some(per_format), Some(total), Some(open_ms)) = (number(0), number(1), number(2)) else {
        return 2;
    };
    let limits = Limits {
        per_format: per_format as usize,
        total: total as usize,
        open_wait: Duration::from_millis(open_ms),
        deadline: Duration::ZERO,
    };
    let mut failed = false;
    read_all(raw, &limits, &mut |event| {
        // Flushed at once, so SuperEditr knows which format a stall is waiting on.
        failed |= wire::write(out, &event).and_then(|()| out.flush()).is_err();
    });
    i32::from(failed)
}

/// Reads a child's events from `input` until it ends, passing them to `events`. A child stopped
/// mid-way ends its output with part of a record, which is dropped.
fn forward(input: impl io::Read, events: Sender<Event>) {
    let mut input = io::BufReader::with_capacity(1 << 16, input);
    while let Ok(Some(event)) = wire::read(&mut input) {
        if events.send(event).is_err() {
            return;
        }
    }
}

/// The system clipboard, through Win32.
pub mod system {
    use super::{Data, Event, Limits, Raw, Read, SNAPSHOT_COMMAND, Stop};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::mpsc::Sender;
    use windows::Win32::Foundation::{GetLastError, GlobalFree, HANDLE, HGLOBAL, HWND};
    use windows::Win32::Graphics::Gdi::{
        GetEnhMetaFileBits, GetMetaFileBitsEx, HENHMETAFILE, HMETAFILE, SetEnhMetaFileBits, SetMetaFileBitsEx,
    };
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardFormatNameW,
        GetClipboardOwner, GetClipboardSequenceNumber, GetOpenClipboardWindow, OpenClipboard, RegisterClipboardFormatW,
        SetClipboardData,
    };
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    use windows::core::w;

    /// The window that owns what SuperEditr writes: the hotkey thread's message-only window, whose
    /// thread answers clipboard messages. None until it exists, and in the snapshot's child.
    pub static OWNER: AtomicIsize = AtomicIsize::new(0);

    #[repr(C)]
    struct MetafilePict {
        mm: i32,
        x: i32,
        y: i32,
        hmf: *mut core::ffi::c_void,
    }

    pub struct System;

    fn global(bytes: &[u8]) -> Option<HANDLE> {
        unsafe {
            let memory: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).ok()?;
            let target = GlobalLock(memory) as *mut u8;
            if target.is_null() {
                let _ = GlobalFree(Some(memory));
                return None;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
            let _ = GlobalUnlock(memory);
            Some(HANDLE(memory.0))
        }
    }

    fn give(format: u32, handle: HANDLE) -> bool {
        unsafe { SetClipboardData(format, Some(handle)) }.is_ok()
    }

    impl Raw for System {
        fn open(&self) -> bool {
            let owner = OWNER.load(Ordering::Relaxed);
            let owner = (owner != 0).then_some(HWND(owner as *mut _));
            unsafe { OpenClipboard(owner) }.is_ok()
        }

        fn close(&self) {
            let _ = unsafe { CloseClipboard() };
        }

        fn sequence(&self) -> u32 {
            unsafe { GetClipboardSequenceNumber() }
        }

        fn formats(&self) -> Vec<u32> {
            let mut all = Vec::new();
            let mut format = 0;
            loop {
                format = unsafe { EnumClipboardFormats(format) };
                if format == 0 {
                    return all;
                }
                all.push(format);
            }
        }

        fn read(&self, format: u32, limit: usize) -> Read {
            unsafe {
                let handle = match GetClipboardData(format) {
                    Ok(handle) if !handle.0.is_null() => handle,
                    Ok(_) => return Read::Absent,
                    Err(error) if error.code().0 == 0 || GetLastError().0 == 0 => return Read::Absent,
                    Err(error) => return Read::Failed(format!("{:#x}", error.code().0)),
                };
                match format {
                    super::CF_ENHMETAFILE | super::CF_DSPENHMETAFILE => {
                        let emf = HENHMETAFILE(handle.0);
                        let n = GetEnhMetaFileBits(emf, None) as usize;
                        if n > limit {
                            return Read::TooLarge(n);
                        }
                        let mut bits = vec![0u8; n];
                        GetEnhMetaFileBits(emf, Some(&mut bits));
                        Read::Data(Data::Emf(bits))
                    }
                    super::CF_METAFILEPICT | super::CF_DSPMETAFILEPICT => {
                        let memory = HGLOBAL(handle.0);
                        let header = GlobalLock(memory) as *const MetafilePict;
                        if header.is_null() {
                            return Read::Failed("GlobalLock".into());
                        }
                        let pict = std::ptr::read(header);
                        let _ = GlobalUnlock(memory);
                        let metafile = HMETAFILE(pict.hmf);
                        let n = GetMetaFileBitsEx(metafile, 0, None) as usize;
                        if n > limit {
                            return Read::TooLarge(n);
                        }
                        let mut bits = vec![0u8; n];
                        GetMetaFileBitsEx(metafile, n as u32, Some(bits.as_mut_ptr() as *mut _));
                        Read::Data(Data::MetafilePict { mm: pict.mm, x: pict.x, y: pict.y, bits })
                    }
                    _ => {
                        let memory = HGLOBAL(handle.0);
                        let n = GlobalSize(memory);
                        if n == 0 {
                            return Read::Failed("not a memory block".into());
                        }
                        if n > limit {
                            return Read::TooLarge(n);
                        }
                        let source = GlobalLock(memory) as *const u8;
                        if source.is_null() {
                            return Read::Failed("GlobalLock".into());
                        }
                        let bytes = std::slice::from_raw_parts(source, n).to_vec();
                        let _ = GlobalUnlock(memory);
                        Read::Data(Data::Bytes(bytes))
                    }
                }
            }
        }

        fn empty(&self) -> bool {
            unsafe { EmptyClipboard() }.is_ok()
        }

        fn set(&self, format: u32, data: &Data) -> bool {
            unsafe {
                match data {
                    Data::Bytes(bytes) => match global(bytes) {
                        Some(handle) => {
                            let ok = give(format, handle);
                            if !ok {
                                let _ = GlobalFree(Some(HGLOBAL(handle.0)));
                            }
                            ok
                        }
                        None => false,
                    },
                    Data::Emf(bits) => {
                        let emf = SetEnhMetaFileBits(bits);
                        !emf.0.is_null() && give(format, HANDLE(emf.0))
                    }
                    Data::MetafilePict { mm, x, y, bits } => {
                        let metafile = SetMetaFileBitsEx(bits);
                        if metafile.0.is_null() {
                            return false;
                        }
                        let pict = MetafilePict { mm: *mm, x: *x, y: *y, hmf: metafile.0 };
                        let raw = std::slice::from_raw_parts(
                            &pict as *const _ as *const u8,
                            std::mem::size_of::<MetafilePict>(),
                        );
                        global(raw).is_some_and(|handle| give(format, handle))
                    }
                }
            }
        }

        fn exclude_format(&self) -> u32 {
            unsafe { RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")) }
        }

        fn html_format(&self) -> u32 {
            unsafe { RegisterClipboardFormatW(w!("HTML Format")) }
        }

        fn holder(&self) -> Option<String> {
            app_of(unsafe { GetOpenClipboardWindow() }.ok())
        }

        fn owner(&self) -> Option<String> {
            app_of(unsafe { GetClipboardOwner() }.ok())
        }

        fn format_name(&self, format: u32) -> String {
            let mut name = [0u16; 128];
            let n = unsafe { GetClipboardFormatNameW(format, &mut name) };
            if n > 0 { String::from_utf16_lossy(&name[..n as usize]) } else { format.to_string() }
        }

        /// Starts SuperEditr's own program as the reader. Stopping it kills the process, and Windows
        /// closes the clipboard of a process that ended, even one blocked in `GetClipboardData`.
        fn start_reader(self: Arc<Self>, limits: Limits, events: Sender<Event>) -> Result<Stop, String> {
            use std::os::windows::process::CommandExt;
            use std::process::{Command, Stdio};
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let program = std::env::current_exe().map_err(|e| format!("no program path: {e}"))?;
            let mut child = Command::new(program)
                .arg(SNAPSHOT_COMMAND)
                .arg(limits.per_format.to_string())
                .arg(limits.total.to_string())
                .arg(limits.open_wait.as_millis().to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .map_err(|e| format!("no reader: {e}"))?;
            let Some(output) = child.stdout.take() else {
                let _ = child.kill();
                let _ = child.wait();
                return Err("no reader output".into());
            };
            let forwarding = std::thread::Builder::new()
                .name("clipboard snapshot".into())
                .spawn(move || super::forward(output, events));
            if forwarding.is_err() {
                let _ = child.kill();
                let _ = child.wait();
                return Err("no reader thread".into());
            }
            Ok(Box::new(move || {
                // Killing a process that already exited does nothing; waiting reaps it. Once it has
                // ended, its clipboard is closed.
                let _ = child.kill();
                let _ = child.wait();
            }))
        }
    }

    /// The program that owns `window`, by its readable name.
    fn app_of(window: Option<HWND>) -> Option<String> {
        let window = window.filter(|w| !w.0.is_null())?;
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
        super::super::window::image_name(pid).map(|name| super::super::apps::display_name(&name))
    }
}

#[cfg(test)]
pub mod fake {
    //! A clipboard in memory, with the ways a real one misbehaves.
    use super::{Data, Event, Limits, Raw, Read, Stop};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::{Arc, Mutex};
    use std::thread::ThreadId;
    use std::time::Duration;

    pub type Hook = Box<dyn FnOnce(&Fake) + Send>;

    #[derive(Default)]
    pub struct Fake {
        pub formats: Mutex<Vec<(u32, Vec<u8>)>>,
        pub sequence: AtomicU32,
        /// Formats whose owner answers with nothing.
        pub absent: Mutex<Vec<u32>>,
        /// A format whose owner takes this long to render it.
        pub stall: Mutex<Option<(u32, Duration)>>,
        /// Another app holds the clipboard open.
        pub held: AtomicBool,
        pub open: AtomicBool,
        pub reads: AtomicUsize,
        /// Every format reads as too large.
        pub too_large: AtomicBool,
        /// Runs during the next read of a format: the user acting while a snapshot is under way.
        pub during_read: Mutex<Option<Hook>>,
        /// Runs once the clipboard is next closed: another app copying as soon as it can.
        pub after_close: Mutex<Option<Hook>>,
        /// How many times SuperEditr emptied the clipboard to write it.
        pub empties: AtomicUsize,
        /// Readers stopped, as a killed process is: what they do afterwards no longer counts.
        pub stopped: Mutex<HashSet<ThreadId>>,
        /// How many readers were stopped before they finished.
        pub kills: AtomicUsize,
        /// The app that put the clipboard's contents there, when not "QuillPoint".
        pub owner: Mutex<Option<String>>,
    }

    impl Fake {
        pub fn with(formats: &[(u32, &[u8])]) -> Self {
            let fake = Fake::default();
            *fake.formats.lock().unwrap() = formats.iter().map(|(f, b)| (*f, b.to_vec())).collect();
            fake.sequence.store(10, Ordering::SeqCst);
            fake
        }

        /// Another app copies text.
        pub fn copy_from_app(&self, text: &str) {
            *self.formats.lock().unwrap() = vec![(super::CF_UNICODETEXT, super::utf16z(text))];
            self.sequence.fetch_add(1, Ordering::SeqCst);
        }

        /// The app named `app` (as [`Raw::owner`] names it) copies text.
        pub fn copy_from(&self, app: &str, text: &str) {
            *self.owner.lock().unwrap() = Some(app.into());
            self.copy_from_app(text);
        }

        pub fn get(&self, format: u32) -> Option<Vec<u8>> {
            self.formats.lock().unwrap().iter().find(|(f, _)| *f == format).map(|(_, b)| b.clone())
        }

        pub fn text(&self) -> Option<String> {
            self.get(super::CF_UNICODETEXT).map(|b| super::from_utf16z(&b))
        }

        fn stopped_here(&self) -> bool {
            self.stopped.lock().unwrap().contains(&std::thread::current().id())
        }
    }

    impl Raw for Fake {
        fn open(&self) -> bool {
            !self.held.load(Ordering::SeqCst) && !self.open.swap(true, Ordering::SeqCst)
        }
        fn close(&self) {
            // A stopped reader's clipboard was closed when it was stopped.
            if self.stopped_here() {
                return;
            }
            self.open.store(false, Ordering::SeqCst);
            let hook = self.after_close.lock().unwrap().take();
            if let Some(hook) = hook {
                hook(self);
            }
        }
        fn sequence(&self) -> u32 {
            self.sequence.load(Ordering::SeqCst)
        }
        fn formats(&self) -> Vec<u32> {
            self.formats.lock().unwrap().iter().map(|(f, _)| *f).collect()
        }
        fn read(&self, format: u32, limit: usize) -> Read {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let hook = self.during_read.lock().unwrap().take();
            if let Some(hook) = hook {
                hook(self);
            }
            if self.too_large.load(Ordering::SeqCst) {
                return Read::TooLarge(300 << 20);
            }
            let stall = *self.stall.lock().unwrap();
            if let Some((stalled, wait)) = stall
                && stalled == format
            {
                std::thread::sleep(wait);
            }
            if self.absent.lock().unwrap().contains(&format) {
                return Read::Absent;
            }
            match self.get(format) {
                Some(bytes) if bytes.len() > limit => Read::TooLarge(bytes.len()),
                Some(bytes) => Read::Data(Data::Bytes(bytes)),
                None => Read::Failed("no such format".into()),
            }
        }
        fn empty(&self) -> bool {
            self.empties.fetch_add(1, Ordering::SeqCst);
            self.formats.lock().unwrap().clear();
            self.sequence.fetch_add(1, Ordering::SeqCst);
            *self.owner.lock().unwrap() = Some("SuperEditr".into());
            true
        }
        fn set(&self, format: u32, data: &Data) -> bool {
            let Data::Bytes(bytes) = data else { return false };
            self.formats.lock().unwrap().push((format, bytes.clone()));
            true
        }
        fn exclude_format(&self) -> u32 {
            0xC0DE
        }
        fn html_format(&self) -> u32 {
            0xC100
        }
        fn holder(&self) -> Option<String> {
            self.held.load(Ordering::SeqCst).then(|| "Kyndal’s Notes".to_string())
        }
        fn owner(&self) -> Option<String> {
            Some(self.owner.lock().unwrap().clone().unwrap_or_else(|| "QuillPoint".into()))
        }
        fn format_name(&self, format: u32) -> String {
            if format == 0xC100 { "HTML Format".into() } else { format.to_string() }
        }
        /// A thread stands in for the child process. Stopping it closes the clipboard at once, as
        /// Windows does for a killed process, and whatever the thread does afterwards is ignored.
        fn start_reader(self: Arc<Self>, limits: Limits, events: Sender<Event>) -> Result<Stop, String> {
            let fake = self.clone();
            let (started, thread) = std::sync::mpsc::channel();
            let finished = Arc::new(AtomicBool::new(false));
            let done = finished.clone();
            std::thread::spawn(move || {
                let _ = started.send(std::thread::current().id());
                super::read_all(&*fake, &limits, &mut |event| {
                    // Before it's sent: once SuperEditr has it, it stops the reader.
                    if matches!(event, Event::Done(_)) {
                        done.store(true, Ordering::SeqCst);
                    }
                    let _ = events.send(event);
                });
            });
            let thread = thread.recv().map_err(|e| e.to_string())?;
            Ok(Box::new(move || {
                if !finished.load(Ordering::SeqCst) {
                    self.stopped.lock().unwrap().insert(thread);
                    self.kills.fetch_add(1, Ordering::SeqCst);
                    self.open.store(false, Ordering::SeqCst);
                }
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Fake;
    use super::*;
    use std::sync::atomic::AtomicBool;

    const HTML: u32 = 0xC100;
    const PRIVATE: u32 = 0xC101;

    fn users_clipboard() -> Clipboard<Fake> {
        Clipboard::new(Fake::with(&[
            (CF_UNICODETEXT, &utf16z("Rhyan's own copy")),
            (HTML, b"<b>Rhyan</b>"),
            (PRIVATE, &[1, 2, 3]),
            (CF_TEXT, b"Rhyan's own copy\0"),
            (CF_OEMTEXT, b"Rhyan's own copy\0"),
        ]))
    }

    fn quick() -> Limits {
        Limits { open_wait: Duration::from_millis(40), deadline: Duration::from_millis(80), ..Limits::default() }
    }

    #[test]
    fn a_snapshot_skips_what_windows_makes_again_and_comes_back_whole() {
        let clipboard = users_clipboard();
        let snapshot = clipboard.snapshot(&quick()).unwrap();
        let ids: Vec<u32> = snapshot.formats.iter().map(|(f, _)| *f).collect();
        assert_eq!(ids, [CF_UNICODETEXT, HTML, PRIVATE], "CF_TEXT and CF_OEMTEXT are made from CF_UNICODETEXT");
        let ours = clipboard.write_text(snapshot.sequence, "They’re going", None).unwrap();
        assert_eq!(clipboard.raw().text().as_deref(), Some("They’re going"));
        assert!(clipboard.raw().get(0xC0DE).is_some(), "kept out of clipboard history");
        assert!(clipboard.raw().get(HTML).is_none(), "no HTML unless asked for");
        assert_eq!(clipboard.restore(&snapshot, ours), Restore::Restored);
        assert_eq!(clipboard.raw().text().as_deref(), Some("Rhyan's own copy"));
        assert_eq!(clipboard.raw().get(HTML).unwrap(), b"<b>Rhyan</b>");
        assert_eq!(clipboard.raw().get(PRIVATE).unwrap(), [1, 2, 3]);
        assert!(!clipboard.raw().open.load(Ordering::SeqCst), "closed");
        assert_eq!(clipboard.raw().kills.load(Ordering::SeqCst), 0, "a reader that finished isn't stopped");
    }

    #[test]
    fn a_bitmap_is_put_aside_as_the_bytes_of_a_dib() {
        let clipboard = Clipboard::new(Fake::with(&[(CF_BITMAP, b""), (CF_DIB, b"pixels"), (CF_DIBV5, b"v5 pixels")]));
        let snapshot = clipboard.snapshot(&quick()).unwrap();
        assert_eq!(snapshot.formats, [(CF_DIB, Data::Bytes(b"pixels".to_vec()))], "Windows makes CF_BITMAP from it");
    }

    #[test]
    fn a_format_over_the_limit_makes_the_snapshot_incomplete() {
        let clipboard = Clipboard::new(Fake::with(&[(CF_DIB, &vec![7u8; 2048])]));
        let limits = Limits { per_format: 1024, ..quick() };
        assert_eq!(clipboard.snapshot(&limits).unwrap_err(), Incomplete::TooLarge { format: CF_DIB, bytes: 2048 });
        let limits = Limits { per_format: 4096, total: 1000, ..quick() };
        assert!(matches!(clipboard.snapshot(&limits), Err(Incomplete::TooLarge { .. })), "the total counts too");
    }

    #[test]
    fn a_format_the_owner_answers_with_nothing_is_absent_not_missing() {
        let clipboard = Clipboard::new(Fake::with(&[(CF_DIB, b"pixels"), (PRIVATE, b"")]));
        clipboard.raw().absent.lock().unwrap().push(PRIVATE);
        let snapshot = clipboard.snapshot(&quick()).unwrap();
        assert_eq!(snapshot.formats.len(), 1);
    }

    #[test]
    fn formats_that_cant_be_copied_make_it_incomplete() {
        for format in [CF_PALETTE, CF_OWNERDISPLAY, CF_DSPBITMAP, 0x305] {
            let clipboard = Clipboard::new(Fake::with(&[(format, b"x")]));
            assert_eq!(clipboard.snapshot(&quick()).unwrap_err(), Incomplete::NotCopyable(format));
        }
    }

    #[test]
    fn a_clipboard_held_open_by_another_app_is_named() {
        let clipboard = users_clipboard();
        clipboard.raw().held.store(true, Ordering::SeqCst);
        assert_eq!(clipboard.snapshot(&quick()).unwrap_err(), Incomplete::Held(Some("Kyndal’s Notes".into())));
    }

    #[test]
    fn a_stalling_owner_times_out_and_the_clipboard_is_free_at_once() {
        let clipboard = users_clipboard();
        *clipboard.raw().stall.lock().unwrap() = Some((HTML, Duration::from_millis(400)));
        let started = Instant::now();
        assert_eq!(clipboard.snapshot(&quick()).unwrap_err(), Incomplete::TimedOut);
        assert!(started.elapsed() < Duration::from_millis(300), "the deadline holds: {:?}", started.elapsed());
        assert_eq!(clipboard.stuck(), "format HTML Format (49408) from QuillPoint", "capture.log can say what stalled");
        assert_eq!(clipboard.raw().kills.load(Ordering::SeqCst), 1, "the reader was stopped");
        // Another app copies while the owner is still rendering for the stopped reader.
        assert!(clipboard.raw().open(), "the clipboard isn't left open");
        clipboard.raw().close();
        // A second try reads again, and stops again; once the owner answers, it's whole.
        assert_eq!(clipboard.snapshot(&quick()).unwrap_err(), Incomplete::TimedOut);
        *clipboard.raw().stall.lock().unwrap() = None;
        let snapshot = clipboard.snapshot(&quick()).unwrap();
        assert_eq!(snapshot.formats.len(), 3);
        std::thread::sleep(Duration::from_millis(500));
        assert!(!clipboard.raw().open.load(Ordering::SeqCst), "the stopped readers closed nothing of anyone else's");
    }

    #[test]
    fn a_standard_format_is_named_in_capture_log() {
        let clipboard = Clipboard::new(Fake::with(&[(CF_UNICODETEXT, &utf16z("Brynlee"))]));
        *clipboard.raw().stall.lock().unwrap() = Some((CF_UNICODETEXT, Duration::from_millis(300)));
        assert_eq!(clipboard.snapshot(&quick()).unwrap_err(), Incomplete::TimedOut);
        assert_eq!(clipboard.stuck(), "format CF_UNICODETEXT (13) from QuillPoint");
        assert_eq!(standard_name(CF_DIB), Some("CF_DIB"));
        assert_eq!(standard_name(CF_DSPENHMETAFILE), Some("CF_DSPENHMETAFILE"));
        assert_eq!(standard_name(0xC100), None, "a registered format keeps its own name");
    }

    #[test]
    fn writing_anyway_replaces_whatever_is_there() {
        let clipboard = users_clipboard();
        clipboard.raw().copy_from_app("Brynlee's newer copy");
        let ours = clipboard.write_text_anyway("They’re going", None).unwrap();
        assert_eq!(ours, clipboard.sequence());
        assert_eq!(clipboard.raw().text().as_deref(), Some("They’re going"));
        assert!(clipboard.raw().get(0xC0DE).is_some(), "kept out of clipboard history");
        clipboard.raw().held.store(true, Ordering::SeqCst);
        assert_eq!(clipboard.write_text_anyway("They’re going", None), Err(WriteError::Held));
    }

    #[test]
    fn a_newer_copy_is_never_overwritten() {
        let clipboard = users_clipboard();
        let snapshot = clipboard.snapshot(&quick()).unwrap();
        clipboard.raw().copy_from_app("Brynlee's newer copy");
        assert_eq!(clipboard.write_text(snapshot.sequence, "They’re going", None), Err(WriteError::Changed));
        assert_eq!(clipboard.raw().text().as_deref(), Some("Brynlee's newer copy"));

        let snapshot = clipboard.snapshot(&quick()).unwrap();
        let ours = clipboard.write_text(snapshot.sequence, "They’re going", None).unwrap();
        clipboard.raw().copy_from_app("Kyndal's copy after the paste");
        assert_eq!(clipboard.restore(&snapshot, ours), Restore::Newer);
        assert_eq!(clipboard.raw().text().as_deref(), Some("Kyndal's copy after the paste"));
    }

    #[test]
    fn events_cross_the_pipe_unchanged() {
        let events = [
            Event::Opened(77),
            Event::Reading(HTML),
            Event::Format(CF_UNICODETEXT, Data::Bytes(utf16z("Kyndal’s notes 👋🏽"))),
            Event::Format(CF_ENHMETAFILE, Data::Emf(vec![1, 2, 3])),
            Event::Format(CF_METAFILEPICT, Data::MetafilePict { mm: 8, x: -1, y: 300, bits: vec![9; 40] }),
            Event::Done(Ok(())),
            Event::Done(Err(Incomplete::Held(Some("Kyndal’s Notes".into())))),
            Event::Done(Err(Incomplete::Held(None))),
            Event::Done(Err(Incomplete::TooLarge { format: CF_DIB, bytes: 300 << 20 })),
            Event::Done(Err(Incomplete::NotCopyable(CF_PALETTE))),
            Event::Done(Err(Incomplete::Failed("0x800401d0".into()))),
        ];
        let mut pipe = Vec::new();
        for event in &events {
            wire::write(&mut pipe, event).unwrap();
        }
        let mut input = pipe.as_slice();
        for event in events {
            assert_eq!(wire::read(&mut input).unwrap(), Some(event));
        }
        assert_eq!(wire::read(&mut input).unwrap(), None);
        let mut cut = &b"F\x0d\x00\x00\x00\x00\xff"[..];
        assert!(wire::read(&mut cut).is_err(), "a record cut short by a killed child is an error, not data");
    }

    #[test]
    fn the_child_reads_the_clipboard_and_ends_its_output_with_how_it_went() {
        let raw = Fake::with(&[(CF_UNICODETEXT, &utf16z("Rhyan")), (CF_TEXT, b"Rhyan\0")]);
        let mut out = Vec::new();
        let args: Vec<String> = ["1024", "4096", "40"].map(String::from).into();
        assert_eq!(serve_snapshot(&raw, &args, &mut out), 0);
        let mut input = out.as_slice();
        let mut events = Vec::new();
        while let Some(event) = wire::read(&mut input).unwrap() {
            events.push(event);
        }
        assert_eq!(
            events,
            [
                Event::Opened(10),
                Event::Reading(CF_UNICODETEXT),
                Event::Format(CF_UNICODETEXT, Data::Bytes(utf16z("Rhyan"))),
                Event::Done(Ok(())),
            ]
        );
        assert_eq!(serve_snapshot(&raw, &["x".into()], &mut Vec::new()), 2, "bad arguments");
    }

    #[test]
    fn a_fix_can_go_on_the_clipboard_with_an_html_version() {
        let clipboard = users_clipboard();
        let snapshot = clipboard.snapshot(&quick()).unwrap();
        let html = html_paragraphs("They’re <going>").unwrap();
        clipboard.write_text(snapshot.sequence, "They’re <going>", Some(&html)).unwrap();
        assert_eq!(clipboard.raw().text().as_deref(), Some("They’re <going>"));
        let written = String::from_utf8(clipboard.raw().get(HTML).unwrap()).unwrap();
        assert!(written.contains("<!--StartFragment-->They’re &lt;going&gt;<!--EndFragment-->"), "{written}");
    }

    #[test]
    fn html_holds_each_line_as_a_paragraph_or_isnt_used() {
        assert_eq!(html_paragraphs("Rhyan & Brynlee").as_deref(), Some("Rhyan &amp; Brynlee"));
        assert_eq!(
            html_paragraphs("Teh fix first.\r\n\r\nSee you soon.").as_deref(),
            Some("<p>Teh fix first.</p><p><br></p><p>See you soon.</p>")
        );
        for lossy in ["the ", " the", "the  notes", "one\ttwo", "line \nnext"] {
            assert_eq!(html_paragraphs(lossy), None, "{lossy:?}: HTML would change its spaces");
        }
    }

    #[test]
    fn cf_html_offsets_point_at_the_fragment() {
        let bytes = cf_html("Kyndal’s <b>notes</b>");
        assert_eq!(bytes.last(), Some(&0));
        let text = std::str::from_utf8(&bytes[..bytes.len() - 1]).unwrap();
        let offset = |key: &str| {
            let at = text.find(key).unwrap() + key.len();
            text[at..at + 10].parse::<usize>().unwrap()
        };
        assert_eq!(&text[offset("StartFragment:")..offset("EndFragment:")], "Kyndal’s <b>notes</b>");
        assert!(text[offset("StartHTML:")..].starts_with("<html>"));
        assert_eq!(offset("EndHTML:"), text.len());
    }

    /// The app the guarded copy sent Ctrl+C to copies after SuperEditr gave up waiting.
    fn given_up_copy(clipboard: &Clipboard<Fake>, copies: usize) {
        given_up_copy_watching(clipboard, copies, Arc::new(AtomicBool::new(false)));
    }

    /// As [`given_up_copy`], with `user_acted` standing in for the user's own copy key.
    fn given_up_copy_watching(clipboard: &Clipboard<Fake>, copies: usize, user_acted: Arc<AtomicBool>) {
        let snapshot = clipboard.snapshot(&quick()).unwrap();
        clipboard.watch_late_copies(
            snapshot,
            "Google Chrome".into(),
            copies,
            Box::new(move || user_acted.load(Ordering::SeqCst)),
        );
    }

    fn wait_for(what: impl Fn() -> bool) -> bool {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            if what() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    #[test]
    fn a_late_copy_is_undone() {
        let clipboard = users_clipboard();
        given_up_copy(&clipboard, 2);
        clipboard.raw().copy_from("Google Chrome", "Their going");
        assert!(wait_for(|| clipboard.raw().text().as_deref() == Some("Rhyan's own copy")), "put back");
        assert_eq!(clipboard.raw().get(HTML).unwrap(), b"<b>Rhyan</b>", "every format comes back");
        // The second Ctrl+C's copy comes later still.
        clipboard.raw().copy_from("Google Chrome", "Their going");
        assert!(wait_for(|| clipboard.raw().text().as_deref() == Some("Rhyan's own copy")), "put back again");
    }

    #[test]
    fn a_copy_from_the_app_after_the_users_own_copy_key_is_theirs_and_stays() {
        let clipboard = users_clipboard();
        let user_acted = Arc::new(AtomicBool::new(false));
        given_up_copy_watching(&clipboard, 2, user_acted.clone());
        // The user selects something else in the same app and presses Ctrl+C: the copy is theirs.
        user_acted.store(true, Ordering::SeqCst);
        clipboard.raw().copy_from("Google Chrome", "Kyndal's own copy");
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(clipboard.raw().text().as_deref(), Some("Kyndal's own copy"));
        // The watch is over, and the next transaction doesn't undo it either.
        clipboard.settle_late();
        assert_eq!(clipboard.raw().text().as_deref(), Some("Kyndal's own copy"));
    }

    #[test]
    fn a_late_copy_with_no_copy_key_from_the_user_is_undone() {
        let clipboard = users_clipboard();
        given_up_copy_watching(&clipboard, 1, Arc::new(AtomicBool::new(false)));
        clipboard.raw().copy_from("Google Chrome", "Their going");
        assert!(wait_for(|| clipboard.raw().text().as_deref() == Some("Rhyan's own copy")), "put back");
    }

    #[test]
    fn a_users_own_copy_after_a_given_up_copy_is_kept() {
        let clipboard = users_clipboard();
        given_up_copy(&clipboard, 2);
        clipboard.raw().copy_from("Notepad", "Brynlee's newer copy");
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(clipboard.raw().text().as_deref(), Some("Brynlee's newer copy"));
        // The watch is over: a copy from the app now is the user's too.
        clipboard.raw().copy_from("Google Chrome", "Kyndal's copy");
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(clipboard.raw().text().as_deref(), Some("Kyndal's copy"));
    }

    #[test]
    fn a_users_copy_after_a_late_copy_was_undone_is_kept() {
        let clipboard = users_clipboard();
        given_up_copy(&clipboard, 1);
        clipboard.raw().copy_from("Google Chrome", "Their going");
        assert!(wait_for(|| clipboard.raw().text().as_deref() == Some("Rhyan's own copy")));
        clipboard.raw().copy_from("Google Chrome", "Kyndal's copy");
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(clipboard.raw().text().as_deref(), Some("Kyndal's copy"), "one Ctrl+C, one copy undone");
    }

    #[test]
    fn a_late_copy_after_the_watch_ends_stays() {
        let clipboard = users_clipboard();
        given_up_copy(&clipboard, 1);
        std::thread::sleep(LATE_WAIT + Duration::from_millis(100));
        clipboard.raw().copy_from("Google Chrome", "Kyndal's copy");
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(clipboard.raw().text().as_deref(), Some("Kyndal's copy"));
    }

    #[test]
    fn a_transaction_right_after_a_late_copy_puts_the_users_clipboard_aside() {
        let clipboard = users_clipboard();
        given_up_copy(&clipboard, 1);
        // The copy lands, and a replacement puts the clipboard aside before the watch looks.
        let late = clipboard.late.lock().unwrap();
        clipboard.raw().copy_from("Google Chrome", "Their going");
        drop(late);
        let snapshot = clipboard.snapshot(&quick()).unwrap();
        let text = snapshot.formats.iter().find(|(f, _)| *f == CF_UNICODETEXT).map(|(_, d)| d);
        assert_eq!(text, Some(&Data::Bytes(utf16z("Rhyan's own copy"))), "the user's clipboard, not the late copy");
    }

    #[test]
    fn text_round_trips_through_utf16() {
        let bytes = utf16z("teh 👋🏽 notes");
        assert_eq!(from_utf16z(&bytes), "teh 👋🏽 notes");
    }
}
