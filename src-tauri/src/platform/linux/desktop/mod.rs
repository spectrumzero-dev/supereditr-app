//! What only the compositor can do: say which window is in front, focus it, place the panel and
//! send it keys. Each desktop (Hyprland, later GNOME and KDE) implements [`Desktop`]; everything
//! that works the same on any Wayland desktop stays in the shared Linux layer.
//!
//! Every call has a time limit, so a frozen compositor never blocks the app. The quick ones (one
//! request and its answer) are synchronous, so the UI's code can ask them too.

mod generic;
pub mod hyprland;
pub mod lua;
pub mod shortcuts;

pub use generic::FallbackDesktop;

use crate::double_press::{ALT, COMMAND, CONTROL, SHIFT};
use crate::platform::Rect;
use async_trait::async_trait;

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
    pub xwayland: bool,
    /// The compositor's tags for it; Omarchy tags terminals `terminal`.
    pub tags: Vec<String>,
}

/// What can be done for a window in this session. Asked per window, since an XWayland window or
/// another workspace can differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    /// A keystroke sent to this window reaches it.
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

impl Shortcut {
    pub fn ctrl(key: &str) -> Self {
        Self { modifiers: CONTROL, key: key.into() }
    }

    /// A single chord from settings, such as "Super+Alt+E"; `None` for anything else, a double
    /// press included.
    pub fn parse(accelerator: &str) -> Option<Self> {
        let accelerator = accelerator.trim();
        if accelerator.is_empty() || accelerator.contains(char::is_whitespace) {
            return None;
        }
        let mut parts: Vec<&str> = accelerator.split('+').collect();
        let key = parts.pop()?;
        let mut modifiers = 0;
        for part in parts {
            modifiers |= match part.to_ascii_lowercase().as_str() {
                "super" | "command" | "cmd" | "meta" => COMMAND,
                "control" | "ctrl" | "cmdorctrl" | "commandorcontrol" => CONTROL,
                "alt" | "option" => ALT,
                "shift" => SHIFT,
                _ => return None,
            };
        }
        let key = match key.to_ascii_lowercase().as_str() {
            "" => return None,
            "enter" | "return" => "Return".to_string(),
            "esc" | "escape" => "Escape".to_string(),
            "space" => "space".to_string(),
            _ if key.chars().count() == 1 => key.to_uppercase(),
            _ => key.to_string(),
        };
        Some(Self { modifiers, key })
    }

    /// For messages: "Super+Alt+E".
    pub fn label(&self) -> String {
        let mut parts: Vec<&str> = [(COMMAND, "Super"), (CONTROL, "Ctrl"), (ALT, "Alt"), (SHIFT, "Shift")]
            .iter()
            .filter(|(bit, _)| self.modifiers & bit != 0)
            .map(|(_, name)| *name)
            .collect();
        parts.push(&self.key);
        parts.join("+")
    }
}

/// A SuperEditr action a shortcut starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Review,
    QuickFix,
}

impl Action {
    /// Its id with the GlobalShortcuts portal, and as `supereditr <id>` on the command line.
    pub fn id(self) -> &'static str {
        match self {
            Self::Review => "review",
            Self::QuickFix => "quick-fix",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Review => "review the selection",
            Self::QuickFix => "fix the selection in place",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [Self::Review, Self::QuickFix].into_iter().find(|action| action.id() == id)
    }
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

/// A running watch. Dropping it stops it.
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

#[async_trait]
pub trait Desktop: Send + Sync {
    fn name(&self) -> &'static str;
    async fn capabilities(&self, window: &WindowRef) -> Capabilities;
    fn front_window(&self) -> Result<WindowRef, DesktopError>;
    /// Calls `on_change` with each window that comes to the front.
    fn watch_front_window(&self, on_change: Box<dyn Fn(WindowRef) + Send + Sync>) -> Subscription;
    /// The pointer, in global logical coordinates.
    fn cursor(&self) -> Result<(f64, f64), DesktopError>;
    /// Puts the panel's window at `at`: where it opens next, or, while it shows, where it moves.
    fn place_overlay(&self, at: Rect, shown: bool) -> Result<(), DesktopError>;
    async fn focus(&self, window: &WindowRef) -> Result<(), DesktopError>;
    /// Gives the panel keyboard focus, for a review started with the shortcut.
    async fn focus_overlay(&self) -> Result<(), DesktopError>;
    async fn send_shortcut(&self, window: &WindowRef, keys: Shortcut) -> Result<(), DesktopError>;
    /// The desktop's lines for the Linux setup page.
    fn setup_status(&self) -> Vec<SetupCheck>;
}

/// Global shortcuts. Separate from [`Desktop`]: a compositor's own binds and a portal session
/// start and end differently.
pub trait ShortcutBackend: Send + Sync {
    /// Binds these shortcuts in place of the ones before. Returns what couldn't be bound, and why.
    fn install(&self, bindings: &[Binding]) -> Vec<String>;
    fn uninstall(&self);
    fn listen(&self, on_fire: Box<dyn Fn(Action) + Send + Sync>) -> Subscription;
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

    #[test]
    fn settings_shortcuts_parse_into_chords() {
        assert_eq!(Shortcut::parse("Super+Alt+E"), Some(Shortcut { modifiers: COMMAND | ALT, key: "E".into() }));
        assert_eq!(
            Shortcut::parse("Super+Control+Alt+e"),
            Some(Shortcut { modifiers: COMMAND | CONTROL | ALT, key: "E".into() })
        );
        assert_eq!(Shortcut::parse("Alt+Enter").unwrap().key, "Return");
        assert_eq!(Shortcut::parse("Super+F5").unwrap().key, "F5");
        assert_eq!(Shortcut::parse("Command+E Command+E"), None, "a double press isn't a chord");
        assert_eq!(Shortcut::parse("Hyper+E"), None);
        assert_eq!(Shortcut::parse(""), None);
        assert_eq!(Shortcut::parse("Super+Control+Alt+E").unwrap().label(), "Super+Ctrl+Alt+E");
    }

    #[test]
    fn actions_have_stable_ids() {
        for action in [Action::Review, Action::QuickFix] {
            assert_eq!(Action::from_id(action.id()), Some(action));
        }
        assert_eq!(Action::from_id("preset"), None);
    }
}
