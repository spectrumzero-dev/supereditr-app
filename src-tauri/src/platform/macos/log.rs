//! capture.log, the diagnostic log of selection reads and writes.

use std::sync::atomic::Ordering;

pub(super) static TRACE_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn log_dir() -> Option<std::path::PathBuf> {
    Some(std::path::Path::new(&std::env::var_os("HOME")?).join("Library/Logs/SuperEditr"))
}

/// Where capture.log is written, for Settings to show.
pub fn log_path() -> Option<std::path::PathBuf> {
    Some(log_dir()?.join("capture.log"))
}

/// Turns capture.log on or off (Settings → General → Keep a diagnostic log). Turning it off
/// deletes the log, so nothing is left behind once a problem has been diagnosed.
pub fn set_trace(on: bool) {
    TRACE_ON.store(on, Ordering::Relaxed);
    if !on && let Some(dir) = log_dir() {
        let _ = std::fs::remove_file(dir.join("capture.log"));
        let _ = std::fs::remove_file(dir.join("capture.old.log"));
    }
}

/// When the diagnostic log is on, appends one line about a selection read or write to
/// ~/Library/Logs/SuperEditr/capture.log, so a failure in a particular app can be diagnosed. It
/// records app IDs, timings and lengths, never text.
pub fn trace(message: std::fmt::Arguments) {
    use std::io::Write;
    if !TRACE_ON.load(Ordering::Relaxed) {
        return;
    }
    let Some(dir) = log_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("capture.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 256 * 1024) {
        let _ = std::fs::rename(&path, dir.join("capture.old.log"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        // In one write, so lines from different threads can't mix: `writeln!` on a file writes each
        // piece of the format separately.
        let _ = file.write_all(format!("{seconds} {message}\n").as_bytes());
    }
}
