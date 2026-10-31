//! Installed apps, for Settings → Privacy's list of apps SuperEditr never reads text in: each
//! app's name and icon, found by bundle ID through Launch Services, and an open panel for adding
//! apps without knowing their bundle IDs.

use serde::Serialize;
use tauri::WebviewWindow;

/// An installed app, as the Privacy page shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub bundle_id: String,
    pub name: String,
    /// A PNG data URL, 64 pixels square, or None if the icon couldn't be drawn.
    pub icon: Option<String>,
}

/// The apps chosen in the open panel: none if the user cancelled.
#[derive(Debug, Default, Serialize)]
pub struct Picked {
    pub apps: Vec<AppInfo>,
    /// Names of chosen apps without a bundle ID, which SuperEditr can't recognize.
    pub skipped: Vec<String>,
}

/// The installed app with this bundle ID, or None if Launch Services knows no such app.
#[cfg(target_os = "macos")]
pub fn find(bundle_id: &str) -> Option<AppInfo> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;
    let url = NSWorkspace::sharedWorkspace().URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))?;
    let (name, icon) = describe(&url)?;
    Some(AppInfo { bundle_id: bundle_id.to_string(), name, icon })
}

#[cfg(not(target_os = "macos"))]
pub fn find(_bundle_id: &str) -> Option<AppInfo> {
    None
}

/// Shows an open panel on /Applications as a sheet on `window` and calls `done` with the apps the
/// user chose. Must run on the main thread.
pub fn pick(window: &WebviewWindow, done: impl FnOnce(Picked) + 'static) {
    #[cfg(target_os = "macos")]
    {
        use block2::RcBlock;
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSModalResponse, NSModalResponseOK, NSOpenPanel, NSWindow};
        use objc2_foundation::{NSArray, NSBundle, NSURL, ns_string};
        use objc2_uniform_type_identifiers::UTTypeApplicationBundle;
        use std::cell::Cell;

        let (Some(mtm), Ok(ptr)) = (MainThreadMarker::new(), window.ns_window()) else {
            return done(Picked::default());
        };
        let ns_window = unsafe { &*(ptr as *const NSWindow) };
        let panel = NSOpenPanel::openPanel(mtm);
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(false);
        panel.setAllowsMultipleSelection(true);
        panel.setAllowedContentTypes(&NSArray::from_slice(&[unsafe { UTTypeApplicationBundle }]));
        panel.setDirectoryURL(Some(&NSURL::fileURLWithPath_isDirectory(ns_string!("/Applications"), true)));
        panel.setMessage(Some(ns_string!("Choose apps where SuperEditr never reads text.")));
        panel.setPrompt(Some(ns_string!("Add")));

        // AppKit calls the handler once, but takes a block it may call again; `done` runs once.
        let done = Cell::new(Some(done));
        let chosen = panel.clone();
        let handler = RcBlock::new(move |response: NSModalResponse| {
            let Some(done) = done.take() else { return };
            let mut picked = Picked::default();
            if response == NSModalResponseOK {
                for url in chosen.URLs() {
                    let Some((name, icon)) = describe(&url) else { continue };
                    match NSBundle::bundleWithURL(&url).and_then(|bundle| bundle.bundleIdentifier()) {
                        Some(id) => picked.apps.push(AppInfo { bundle_id: id.to_string(), name, icon }),
                        None => picked.skipped.push(name),
                    }
                }
            }
            done(picked);
        });
        panel.beginSheetModalForWindow_completionHandler(ns_window, &handler);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        done(Picked::default());
    }
}

/// The name Finder shows for the app at `url`, without ".app", and its icon.
#[cfg(target_os = "macos")]
fn describe(url: &objc2_foundation::NSURL) -> Option<(String, Option<String>)> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSFileManager;
    let path = url.path()?;
    let name = NSFileManager::defaultManager().displayNameAtPath(&path).to_string();
    let name = name.strip_suffix(".app").map(str::to_string).unwrap_or(name);
    Some((name, icon_data_url(&NSWorkspace::sharedWorkspace().iconForFile(&path))))
}

/// The icon drawn into a 64-pixel square, enough for 32 points on a Retina display, as a PNG data
/// URL.
#[cfg(target_os = "macos")]
fn icon_data_url(icon: &objc2_app_kit::NSImage) -> Option<String> {
    use objc2::AllocAnyThread;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace, NSGraphicsContext,
    };
    use objc2_foundation::{NSDataBase64EncodingOptions, NSDictionary, NSPoint, NSRect, NSSize};
    const PIXELS: isize = 64;
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            PIXELS,
            PIXELS,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    let square = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(PIXELS as f64, PIXELS as f64));
    icon.drawInRect_fromRect_operation_fraction(square, NSRect::ZERO, NSCompositingOperation::SourceOver, 1.0);
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    let png = unsafe { bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }?;
    let base64 = png.base64EncodedStringWithOptions(NSDataBase64EncodingOptions(0));
    Some(format!("data:image/png;base64,{base64}"))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::find;

    #[test]
    fn finds_an_installed_app_by_bundle_id() {
        let finder = find("com.apple.finder").expect("Finder is always installed");
        assert_eq!(finder.name, "Finder");
        assert_eq!(finder.bundle_id, "com.apple.finder");
        assert!(finder.icon.is_some_and(|icon| icon.starts_with("data:image/png;base64,")));
    }

    #[test]
    fn an_app_that_isnt_installed_isnt_found() {
        assert!(find("com.example.quillpoint-not-installed").is_none());
    }
}
