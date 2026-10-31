//! A desktop SuperEditr has no integration for. It can't tell which window is in front, place the
//! panel or send keys, so every suggestion goes to the clipboard, for the user to paste.

use super::{
    Capabilities, Desktop, DesktopError, Output, ReviewKeyGrab, SetupCheck, Shortcut, Subscription, WindowRef,
};
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

    async fn front_window(&self) -> Result<WindowRef, DesktopError> {
        Err(DesktopError::Unsupported)
    }

    fn watch_front_window(&self, _on_change: Box<dyn Fn(WindowRef) + Send + Sync>) -> Subscription {
        Subscription::none()
    }

    async fn cursor(&self) -> Result<(f64, f64, Output), DesktopError> {
        Err(DesktopError::Unsupported)
    }

    async fn place_overlay(
        &self,
        _overlay: &tauri::WebviewWindow,
        _at: Rect,
        _output: &Output,
    ) -> Result<(), DesktopError> {
        Err(DesktopError::Unsupported)
    }

    async fn focus(&self, _window: &WindowRef) -> Result<(), DesktopError> {
        Err(DesktopError::Unsupported)
    }

    async fn send_shortcut(&self, _window: &WindowRef, _keys: Shortcut) -> Result<(), DesktopError> {
        Err(DesktopError::Unsupported)
    }

    async fn grab_review_keys(&self, _keys: &[Shortcut]) -> Result<ReviewKeyGrab, DesktopError> {
        Err(DesktopError::Unsupported)
    }

    fn setup_status(&self) -> Vec<SetupCheck> {
        vec![SetupCheck {
            name: "Desktop".into(),
            ok: false,
            detail: "SuperEditr can’t place its panel or send keys on this desktop, so suggestions go to the clipboard: paste them with Ctrl+V.".into(),
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
            output: None,
            xwayland: false,
        }
    }

    #[test]
    fn the_fallback_can_do_nothing_but_say_so() {
        tauri::async_runtime::block_on(async {
            let desktop = FallbackDesktop;
            assert_eq!(desktop.capabilities(&window()).await, Capabilities::default());
            assert!(matches!(desktop.front_window().await, Err(DesktopError::Unsupported)));
            assert!(matches!(desktop.focus(&window()).await, Err(DesktopError::Unsupported)));
            let paste = Shortcut { modifiers: crate::double_press::CONTROL, key: "V".into() };
            assert!(matches!(desktop.send_shortcut(&window(), paste).await, Err(DesktopError::Unsupported)));
            assert!(desktop.setup_status().iter().all(|check| !check.ok));
        });
    }
}
