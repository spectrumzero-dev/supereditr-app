//! What happens when a window's WebView2 processes stop.
//!
//! WebView2 draws each page in processes of its own: one browser process for all of SuperEditr's
//! windows, and a renderer for each page. A crash, a user ending them in Task Manager, or an update
//! of the runtime can stop them while SuperEditr runs. A window then stays on screen but shows
//! nothing and answers nothing: the panel would take the keyboard and show an empty, invisible card,
//! and Settings would be blank. WebView2 starts a stopped GPU or utility process again by itself, but
//! not the browser process or a page's renderer. So a stopped renderer gets its page loaded again,
//! and a stopped browser process means the window is made again, which starts a new one.

use tauri::WebviewWindow;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_PROCESS_FAILED_KIND, COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
};
use webview2_com::ProcessFailedEventHandler;

/// Which of a window's processes stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// The browser process: the window's webview is gone for good, and the window must be made again.
    Browser,
    /// The page's renderer, which stopped or stopped answering: its page has been loaded again.
    Renderer,
}

/// Calls `on_stopped` on the main thread when `window`'s browser process or renderer stops. A stopped
/// renderer's page is loaded again first.
pub fn on_stopped(window: &WebviewWindow, on_stopped: impl Fn(Stopped) + Send + 'static) {
    let label = window.label().to_string();
    let watched = window.with_webview(move |webview| {
        let core = match unsafe { webview.controller().CoreWebView2() } {
            Ok(core) => core,
            Err(error) => return super::trace(format_args!("webview: can't watch {label}'s processes ({error})")),
        };
        let handler = ProcessFailedEventHandler::create(Box::new(move |sender, args| {
            let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
            if let Some(args) = args {
                unsafe { args.ProcessFailedKind(&mut kind)? };
            }
            let stopped = match kind {
                COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED => Stopped::Browser,
                COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
                | COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE => {
                    if let Some(sender) = sender {
                        let _ = unsafe { sender.Reload() };
                    }
                    Stopped::Renderer
                }
                // The GPU and utility processes, and a frame's renderer, which WebView2 starts again.
                other => {
                    super::trace(format_args!("webview: a WebView2 process stopped (kind {})", other.0));
                    return Ok(());
                }
            };
            on_stopped(stopped);
            Ok(())
        }));
        let mut token = 0;
        if let Err(error) = unsafe { core.add_ProcessFailed(&handler, &mut token) } {
            super::trace(format_args!("webview: can't watch {label}'s processes ({error})"));
        }
    });
    if let Err(error) = watched {
        super::trace(format_args!("webview: can't watch {}'s processes ({error})", window.label()));
    }
}
