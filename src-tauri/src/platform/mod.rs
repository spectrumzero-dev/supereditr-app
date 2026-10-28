//! Operating-system integration: reading the user's selection, replacing it, and noticing new
//! selections. Everything above this module is platform-independent.
//!
//! macOS is implemented with the Accessibility API, Core Graphics events and NSPasteboard. Another
//! platform implements the same interface, for example with UI Automation's TextPattern on Windows.

use serde::Serialize;
use std::time::Duration;

pub mod gesture;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(not(target_os = "macos"))]
mod unsupported;
#[cfg(not(target_os = "macos"))]
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
    #[error("SuperEditr needs Accessibility permission to read and replace text.")]
    NotTrusted,
    #[error("Select some text first.")]
    NoSelection,
    #[error("SuperEditr doesn’t read password fields.")]
    SecureField,
    #[error("The text changed since it was checked. Select it again.")]
    Stale,
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    #[error("Couldn’t replace the text in this app. The suggestion is on your clipboard.")]
    ReplaceFailed,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[error(
        "SuperEditr can’t confirm where text goes in this app, so the suggestion is on your clipboard. Paste it with ⌘V."
    )]
    CopyOnly,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[error("SuperEditr couldn’t confirm the change. Check the text; the suggestion is on your clipboard.")]
    Unverified,
    #[error("{0}")]
    Other(String),
}

/// Something the selection watcher noticed in another app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEvent {
    /// A selection may be finished: review it if nothing else happens within `settle`.
    MaybeSelection { settle: Duration },
    /// The user typed or clicked elsewhere; transient UI should get out of the way.
    Interaction,
    /// The user may be about to continue a selection: hold off a review that hasn't started.
    Wait,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplaceMethod {
    Accessibility,
    Paste,
}
