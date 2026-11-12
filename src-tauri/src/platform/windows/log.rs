//! capture.log, in `%LOCALAPPDATA%\SuperEditr\Logs\`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

static TRACE_ON: AtomicBool = AtomicBool::new(false);

/// The log folder under `local_app_data`, the per-user, per-machine app data folder.
fn log_dir_in(local_app_data: Option<&Path>) -> Option<PathBuf> {
    Some(local_app_data.filter(|dir| dir.is_absolute())?.join(r"SuperEditr\Logs"))
}

fn log_dir() -> Option<PathBuf> {
    log_dir_in(std::env::var_os("LOCALAPPDATA").as_deref().map(Path::new))
}

/// Where capture.log is written, for Settings to show.
pub fn log_path() -> Option<PathBuf> {
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

/// Appends a line to capture.log, when it's on. Lines hold app IDs, timings and lengths, never
/// the text itself.
pub fn trace(message: std::fmt::Arguments) {
    if !TRACE_ON.load(Ordering::Relaxed) {
        return;
    }
    let Some(dir) = log_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("capture.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 256 * 1024) {
        let _ = std::fs::rename(&path, dir.join("capture.old.log"));
    }
    // Milliseconds, to time a review's steps against each other.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let _ = append_line(&path, format_args!("{}.{:03} {message}", now.as_secs(), now.subsec_millis()));
}

/// Adds a line in one write, so lines from different threads can't mix.
fn append_line(path: &Path, line: std::fmt::Arguments) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(format!("{line}\r\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::log_dir_in;
    use std::path::{Path, PathBuf};

    #[test]
    fn the_log_is_in_local_app_data() {
        assert_eq!(
            log_dir_in(Some(Path::new(r"C:\Users\rhyan\AppData\Local"))),
            Some(PathBuf::from(r"C:\Users\rhyan\AppData\Local\SuperEditr\Logs"))
        );
        assert_eq!(log_dir_in(Some(Path::new("relative"))), None);
        assert_eq!(log_dir_in(None), None);
    }
}
