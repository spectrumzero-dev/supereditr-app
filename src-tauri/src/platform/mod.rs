//! Operating-system integration: reading the user's selection, replacing it, and noticing new
//! selections. Everything above this module is platform-independent.
//!
//! macOS is implemented with the Accessibility API, Core Graphics events and NSPasteboard, Linux
//! in `linux/`, and Windows in `windows/`, so far a skeleton that reads and writes nothing.

use serde::Serialize;
use std::time::Duration;

pub mod gesture;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod unsupported;
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub use unsupported::*;

/// Screen rectangle in points, origin at the top-left of the primary display.
#[derive(Debug, Clone, Copy, Serialize, serde::Deserialize, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[error("SuperEditr needs Accessibility permission to read and replace text.")]
    NotTrusted,
    #[error("Select some text first.")]
    NoSelection,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[error("SuperEditr doesn’t read password fields.")]
    SecureField,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[error("The text changed since it was checked. Select it again.")]
    Stale,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    #[error("Couldn’t replace the text in this app. The suggestion is on your clipboard.")]
    ReplaceFailed,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[cfg_attr(
        target_os = "macos",
        error(
            "SuperEditr can’t confirm where text goes in this app, so the suggestion is on your clipboard. Paste it with ⌘V."
        )
    )]
    #[cfg_attr(
        not(target_os = "macos"),
        error(
            "SuperEditr can’t confirm where text goes in this app, so the suggestion is on your clipboard. Paste it with Ctrl+V."
        )
    )]
    CopyOnly,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[error("SuperEditr couldn’t confirm the change. Check the text before copying the suggestion.")]
    Unverified,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[error(
        "SuperEditr applied {applied} of {total} fixes, then stopped because the text no longer matched. Check the text before copying the full suggestion."
    )]
    Partial { applied: usize, total: usize },
    /// Keyboard focus didn't go back to the field the text came from after the panel had it, so
    /// nothing was written: a paste could have landed somewhere else.
    #[error("SuperEditr couldn’t get back to the text you selected, so the suggestion is on your clipboard.")]
    FocusNotReturned,
    /// An input method is still composing text in the field. Taking focus from it would commit the
    /// composition, so the panel doesn't.
    #[cfg_attr(not(windows), allow(dead_code))]
    #[error("Finish what you’re typing first: your input method is still composing it.")]
    Composing,
    /// The clipboard couldn't be put aside to paste through, so nothing was written.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    #[error("{0}")]
    ClipboardNotKept(String),
    /// Like `ClipboardNotKept`, from a platform whose panel then offers to replace anyway, leaving
    /// the fix on the clipboard (Windows). The message is for where no panel can offer that.
    #[cfg_attr(not(windows), allow(dead_code))]
    #[error("{0}")]
    ClipboardUnrestorable(String),
    #[error("{0}")]
    Other(String),
}

/// What happened to the window a review belongs to (see `crate::follow`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowChange {
    /// It started moving or being resized. The panel stays where it is meanwhile.
    Moving,
    /// It moved or was resized and has settled; its frame is now this. The panel glides after it.
    Moved(Rect),
    /// It was minimised, hidden or closed.
    Gone,
}

/// Watches the window a capture's text is in, until dropped. A platform without a way to watch
/// another app's window has none, and the panel stays where it opened.
#[cfg(not(any(target_os = "macos", windows)))]
#[allow(dead_code)]
pub struct WindowWatch;

#[cfg(not(any(target_os = "macos", windows)))]
#[allow(dead_code)]
impl WindowWatch {
    /// The window's frame now, when the platform can read it cheaply.
    pub fn frame(&self) -> Option<Rect> {
        None
    }
}

/// Starts watching the window `capture` came from, calling `on_change` when it moves or goes, and
/// returns the watch with the window's frame now.
#[cfg(not(any(target_os = "macos", windows)))]
pub fn watch_window(
    _capture: &Capture,
    _on_change: impl Fn(WindowChange) + Send + 'static,
) -> Option<(WindowWatch, Rect)> {
    None
}

/// Whether the mouse press the selection watcher just reported landed on the window at `frame` of
/// app `pid`. Asked once per press.
#[cfg(not(any(target_os = "macos", windows)))]
pub fn pressed_on_window(_pid: i32, _frame: Rect) -> bool {
    false
}

/// Whether the left mouse button is down.
#[cfg(not(any(target_os = "macos", windows)))]
pub fn mouse_button_down() -> bool {
    false
}

/// Something the selection watcher noticed in another app.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEvent {
    /// A selection may be finished: review it if nothing else happens within `settle`.
    MaybeSelection { settle: Duration, gesture: SelectionGesture },
    /// The user typed or clicked elsewhere; transient UI should get out of the way.
    Interaction,
    /// The user may be about to continue a selection: hold off a review that hasn't started.
    Wait,
}

/// How the user made a selection, for capture.log.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionGesture {
    Drag,
    DoubleClick,
    SelectAll,
    /// Shift with the arrow keys, Home/End or Page Up/Down, or a Shift-click.
    Shift,
}

impl std::fmt::Display for SelectionGesture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Drag => "drag",
            Self::DoubleClick => "double-click",
            Self::SelectAll => "⌘A",
            Self::Shift => "Shift selection",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplaceMethod {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Accessibility,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Paste,
    /// Typed as key events, for a proofread applied in place in a web field.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Typing,
}

/// How an attempt to replace the selection ended. Every attempt ends in exactly one of these, and
/// each tells the caller what to do with the suggestion.
#[derive(Debug)]
pub enum ReplaceOutcome {
    /// The field now holds the new text.
    Applied,
    /// Nothing was written. The suggestion goes on the clipboard, and the message says so.
    CopyOnly(PlatformError),
    /// A write may have landed but couldn't be confirmed. The suggestion stays on screen to copy;
    /// the clipboard is left alone and nothing is tried again, so the text is never written twice.
    Uncertain(PlatformError),
    /// Nothing was written, because the user's clipboard couldn't be put aside and restored. The
    /// suggestion stays on screen to copy; the clipboard is left alone.
    ClipboardKept(PlatformError),
}

impl From<Result<ReplaceMethod, PlatformError>> for ReplaceOutcome {
    fn from(result: Result<ReplaceMethod, PlatformError>) -> Self {
        match result {
            Ok(_) => Self::Applied,
            Err(error @ (PlatformError::Unverified | PlatformError::Partial { .. })) => Self::Uncertain(error),
            Err(error @ (PlatformError::ClipboardNotKept(_) | PlatformError::ClipboardUnrestorable(_))) => {
                Self::ClipboardKept(error)
            }
            Err(error) => Self::CopyOnly(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_write_that_may_have_landed_is_uncertain() {
        assert!(matches!(ReplaceOutcome::from(Err(PlatformError::Unverified)), ReplaceOutcome::Uncertain(_)));
        assert!(matches!(
            ReplaceOutcome::from(Err(PlatformError::Partial { applied: 2, total: 3 })),
            ReplaceOutcome::Uncertain(_)
        ));
    }

    #[test]
    fn a_clipboard_that_couldnt_be_put_aside_is_kept() {
        let error = PlatformError::ClipboardNotKept("image/png".into());
        assert!(matches!(ReplaceOutcome::from(Err(error)), ReplaceOutcome::ClipboardKept(_)));
        let error = PlatformError::ClipboardUnrestorable("Notes isn’t answering".into());
        assert!(matches!(
            ReplaceOutcome::from(Err(error)),
            ReplaceOutcome::ClipboardKept(PlatformError::ClipboardUnrestorable(_))
        ));
    }

    #[test]
    fn a_refusal_before_writing_is_copy_only() {
        for error in [
            PlatformError::NotTrusted,
            PlatformError::Stale,
            PlatformError::CopyOnly,
            PlatformError::ReplaceFailed,
            PlatformError::FocusNotReturned,
            PlatformError::Other("Brynlee’s editor closed.".into()),
        ] {
            assert!(matches!(ReplaceOutcome::from(Err(error)), ReplaceOutcome::CopyOnly(_)));
        }
    }

    #[test]
    fn a_verified_write_is_applied() {
        assert!(matches!(ReplaceOutcome::from(Ok(ReplaceMethod::Paste)), ReplaceOutcome::Applied));
    }
}
