//! A desktop SuperEditr has no integration for. It can't tell which window is in front, place the
//! panel or send keys, so reviews don't work there; the setup page says so.

use super::{Capabilities, Desktop, DesktopError, SetupCheck, Shortcut, Subscription, WindowRef};
use crate::platform::Rect;
use async_trait::async_trait;

pub struct FallbackDesktop;

#[async_trait]
impl Desktop for FallbackDesktop {
    fn name(&self) -> &'static str {
        "other"
    }

    async fn capabilities(&self, _window: &WindowRef) -> Capabilities {
        Capabilities::default()
    }

    fn front_window(&self) -> Result<WindowRef, DesktopError> {
        Err(DesktopError::Unsupported)
    }

    fn watch_front_window(&self, _on_change: Box<dyn Fn(WindowRef) + Send + Sync>) -> Subscription {
        Subscription::none()
    }

    fn cursor(&self) -> Result<(f64, f64), DesktopError> {
        Err(DesktopError::Unsupported)
    }

    fn place_overlay(&self, _at: Rect, _shown: bool) -> Result<(), DesktopError> {
        Err(DesktopError::Unsupported)
    }

    async fn focus(&self, _window: &WindowRef) -> Result<(), DesktopError> {
        Err(DesktopError::Unsupported)
    }

    async fn focus_overlay(&self) -> Result<(), DesktopError> {
        Err(DesktopError::Unsupported)
    }

    async fn send_shortcut(&self, _window: &WindowRef, _keys: Shortcut) -> Result<(), DesktopError> {
        Err(DesktopError::Unsupported)
    }

    fn setup_status(&self) -> Vec<SetupCheck> {
        vec![SetupCheck {
            name: "Desktop".into(),
            ok: false,
            detail:
                "SuperEditr works with Hyprland, as on Omarchy. On this desktop its shortcuts and panel don’t work yet."
                    .into(),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> WindowRef {
        WindowRef {
            handle: "0x1234".into(),
            pid: 1234,
            app_id: "org.gnome.TextEditor".into(),
            title: "Notes".into(),
            rect: None,
            xwayland: false,
            tags: Vec::new(),
        }
    }

    #[test]
    fn the_fallback_can_do_nothing_but_say_so() {
        tauri::async_runtime::block_on(async {
            let desktop = FallbackDesktop;
            assert_eq!(desktop.capabilities(&window()).await, Capabilities::default());
            assert!(matches!(desktop.front_window(), Err(DesktopError::Unsupported)));
            assert!(matches!(desktop.focus(&window()).await, Err(DesktopError::Unsupported)));
            let paste = Shortcut::ctrl("V");
            assert!(matches!(desktop.send_shortcut(&window(), paste).await, Err(DesktopError::Unsupported)));
            assert!(desktop.setup_status().iter().all(|check| !check.ok));
        });
    }
}
