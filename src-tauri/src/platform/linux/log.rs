//! capture.log, in `$XDG_STATE_HOME/supereditr/` (`~/.local/state/supereditr/` by default).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

static TRACE_ON: AtomicBool = AtomicBool::new(false);

/// The XDG state directory: `$XDG_STATE_HOME` when it's an absolute path, as the XDG Base Directory
/// specification requires, otherwise `~/.local/state`.
fn state_dir(xdg_state_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    match xdg_state_home.filter(|dir| Path::new(dir).is_absolute()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => Some(Path::new(home.filter(|home| !home.is_empty())?).join(".local/state")),
    }
}

fn log_dir() -> Option<PathBuf> {
    let xdg = std::env::var("XDG_STATE_HOME").ok();
    let home = std::env::var("HOME").ok();
    Some(state_dir(xdg.as_deref(), home.as_deref())?.join("supereditr"))
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
        let _ = writeln!(file, "{seconds} {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::state_dir;
    use std::path::PathBuf;

    #[test]
    fn the_log_follows_xdg_state_home() {
        assert_eq!(state_dir(Some("/var/state/rhyan"), Some("/home/rhyan")), Some(PathBuf::from("/var/state/rhyan")));
    }

    #[test]
    fn without_an_absolute_xdg_state_home_the_log_is_in_local_state() {
        let default = Some(PathBuf::from("/home/rhyan/.local/state"));
        assert_eq!(state_dir(None, Some("/home/rhyan")), default);
        assert_eq!(state_dir(Some(""), Some("/home/rhyan")), default);
        assert_eq!(state_dir(Some("relative/state"), Some("/home/rhyan")), default);
        assert_eq!(state_dir(None, None), None);
    }
}
