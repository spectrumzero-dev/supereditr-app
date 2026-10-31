//! What only the compositor can do: say which window is in front, focus it, place the panel and
//! send it keys. Each desktop (Hyprland, later GNOME and KDE) implements [`Desktop`]; everything
//! that works the same on any Wayland desktop stays in the shared Linux layer.
//!
//! Every call has a time limit, so a frozen compositor never blocks the app.
//!
//! The capture and replace chains are the callers of most of this interface; until they exist,
//! only detection and the setup checks are used.
#![allow(dead_code)]

mod generic;

pub use generic::FallbackDesktop;

use crate::platform::Rect;
use async_trait::async_trait;

/// A monitor, in global logical coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub name: String,
    pub scale: f64,
    pub rect: Rect,
}

/// A specific window. One process can own several windows, so its pid alone isn't an identity.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowRef {
    /// Desktop-specific and stable, such as Hyprland's window address.
    pub handle: String,
    pub pid: i32,
    /// The Wayland app ID (Hyprland's window class), such as "foot" or "chromium".
    pub app_id: String,
    pub title: String,
    /// In global logical coordinates.
    pub rect: Option<Rect>,
    pub output: Option<Output>,
    pub xwayland: bool,
}

/// What can be done for a window in this session. Asked per window, since an XWayland window or
/// another workspace can differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub place_overlay: bool,
    /// A paste keystroke reaches this window.
    pub send_keys: bool,
    pub focus: bool,
}

/// A key with its modifiers (`double_press::{COMMAND, CONTROL, ALT, SHIFT}`; COMMAND is Super).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: u8,
    /// A key name, such as "V", "E" or "Insert".
    pub key: String,
}

/// A SuperEditr action a shortcut starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Review,
    QuickFix,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub action: Action,
    pub shortcut: Shortcut,
}

/// One line of the Linux setup page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupCheck {
    pub name: String,
    pub ok: bool,
    /// What was found, or what to do about it.
    pub detail: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    #[error("this desktop can’t do that")]
    Unsupported,
    #[error("the desktop didn’t answer in time")]
    Timeout,
    #[error("{0}")]
    Failed(String),
}

/// A running watch or grab. Dropping it stops it.
#[must_use]
pub struct Subscription(Option<Box<dyn FnOnce() + Send>>);

impl Subscription {
    pub fn new(stop: impl FnOnce() + Send + 'static) -> Self {
        Self(Some(Box::new(stop)))
    }

    /// For a watch that never started: dropping it does nothing.
    pub fn none() -> Self {
        Self(None)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(stop) = self.0.take() {
            stop();
        }
    }
}

/// The panel's keys while it shows, taken from the target app. The grab ends when this drops: on
/// dismiss, timeout or panic.
pub struct ReviewKeyGrab(pub Subscription);

#[async_trait]
pub trait Desktop: Send + Sync {
    fn name(&self) -> &'static str;
    async fn capabilities(&self, window: &WindowRef) -> Capabilities;
    async fn front_window(&self) -> Result<WindowRef, DesktopError>;
    /// Calls `on_change` with each window that comes to the front.
    fn watch_front_window(&self, on_change: Box<dyn Fn(WindowRef) + Send + Sync>) -> Subscription;
    /// The pointer, in global logical coordinates, and the monitor it's on.
    async fn cursor(&self) -> Result<(f64, f64, Output), DesktopError>;
    async fn place_overlay(
        &self,
        overlay: &tauri::WebviewWindow,
        at: Rect,
        output: &Output,
    ) -> Result<(), DesktopError>;
    async fn focus(&self, window: &WindowRef) -> Result<(), DesktopError>;
    async fn send_shortcut(&self, window: &WindowRef, keys: Shortcut) -> Result<(), DesktopError>;
    async fn grab_review_keys(&self, keys: &[Shortcut]) -> Result<ReviewKeyGrab, DesktopError>;
    /// The desktop's lines for the Linux setup page.
    fn setup_status(&self) -> Vec<SetupCheck>;
}

/// Global shortcuts. Separate from [`Desktop`]: a compositor's own binds and a portal session
/// start and end differently.
pub trait ShortcutBackend: Send + Sync {
    fn install(&self, bindings: &[Binding]) -> Result<(), DesktopError>;
    fn uninstall(&self) -> Result<(), DesktopError>;
    fn listen(&self, on_fire: Box<dyn Fn(Action) + Send + Sync>) -> Subscription;
}

/// The desktop this session runs, from its environment. Only the fallback exists so far.
pub fn detect() -> Box<dyn Desktop> {
    Box::new(FallbackDesktop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn dropping_a_subscription_stops_it_once() {
        let stopped = Arc::new(AtomicBool::new(false));
        let flag = stopped.clone();
        let subscription = Subscription::new(move || assert!(!flag.swap(true, Ordering::Relaxed)));
        assert!(!stopped.load(Ordering::Relaxed));
        drop(subscription);
        assert!(stopped.load(Ordering::Relaxed));
    }
}
