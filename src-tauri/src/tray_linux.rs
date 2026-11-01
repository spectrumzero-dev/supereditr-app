//! The tray icon on Linux, served as a StatusNotifierItem.
//!
//! Tauri's tray goes through libayatana-appindicator, which marks no item as menu-only and ignores
//! `Activate`, so on Omarchy's bar a left click did nothing. Here a left click opens Settings, and
//! a right click shows the menu.

use crate::{app_toggle, platform, review, show_settings};
use ksni::menu::{MenuItem, StandardItem};
use ksni::{Icon, ToolTip, Tray, TrayMethods};
use tauri::{AppHandle, Manager};

struct SuperEditrTray {
    app: AppHandle,
    /// The menu's first line: which AI reviews.
    ai_status: String,
    icon: Vec<Icon>,
}

/// The running tray, for updates.
struct TrayHandle(ksni::Handle<SuperEditrTray>);

impl Tray for SuperEditrTray {
    fn id(&self) -> String {
        "supereditr".into()
    }

    fn title(&self) -> String {
        "SuperEditr".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        self.icon.clone()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip { title: "SuperEditr".into(), description: self.ai_status.clone(), ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        show_settings(&self.app, None);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let item = |label: &str, run: fn(&AppHandle)| {
            StandardItem {
                label: label.replace('_', "__"),
                activate: Box::new(move |tray: &mut Self| run(&tray.app)),
                ..Default::default()
            }
            .into()
        };
        let mut menu = vec![
            StandardItem { label: self.ai_status.replace('_', "__"), enabled: false, ..Default::default() }.into(),
            MenuItem::Separator,
            item("Review Selection", review::on_review_hotkey),
        ];
        if let Some(label) = app_toggle::current_label(&self.app) {
            menu.push(item(&label, app_toggle::on_menu));
        }
        menu.extend([
            MenuItem::Separator,
            item("Open Settings", |app| show_settings(app, None)),
            MenuItem::Separator,
            item("Quit SuperEditr", |app| {
                // SuperEditr's binds and window rule live in the compositor until taken out.
                platform::stop();
                app.exit(0)
            }),
        ]);
        menu
    }
}

/// The glyph as the StatusNotifierItem spec wants it: ARGB, one byte each, alpha first.
fn pixmap(png: &[u8]) -> Vec<Icon> {
    let Ok(image) = tauri::image::Image::from_bytes(png) else { return Vec::new() };
    let data = image.rgba().as_chunks::<4>().0.iter().flat_map(|&[r, g, b, a]| [a, r, g, b]).collect();
    vec![Icon { width: image.width() as i32, height: image.height() as i32, data }]
}

pub fn start(app: &AppHandle, png: &'static [u8]) {
    let tray = SuperEditrTray { app: app.clone(), ai_status: "AI: checking…".into(), icon: pixmap(png) };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match tray.spawn().await {
            Ok(handle) => {
                app.manage(TrayHandle(handle));
                // The first check may have finished before the tray was there to show it.
                crate::show_ai_status(&app);
            }
            Err(error) => eprintln!("SuperEditr: couldn’t show the tray icon: {error}"),
        }
    });
}

/// Shows a new first line, or a new "Turn off in…" item.
pub fn refresh(app: &AppHandle, ai_status: Option<String>) {
    let Some(handle) = app.try_state::<TrayHandle>().map(|h| h.0.clone()) else { return };
    tauri::async_runtime::spawn(async move {
        handle
            .update(|tray| {
                if let Some(status) = ai_status {
                    tray.ai_status = status;
                }
            })
            .await;
    });
}

#[cfg(test)]
mod tests {
    use super::pixmap;

    #[test]
    fn the_glyph_becomes_argb_with_alpha_first() {
        let icons = pixmap(include_bytes!("../icons/tray-light.png"));
        let [icon] = icons.as_slice() else { panic!("one size") };
        assert_eq!((icon.width, icon.height), (36, 36));
        assert_eq!(icon.data.len(), 36 * 36 * 4);
        // The light glyph is white wherever it shows.
        assert!(icon.data.as_chunks::<4>().0.iter().filter(|p| p[0] > 0).all(|p| p[1..] == [255, 255, 255]));
    }
}
