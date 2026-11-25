//! The opt-in capture log of app IDs, timings and lengths.

use super::{lock, log_dir};
use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const CURRENT: &str = "capture.log";
const ROTATED: &str = "capture.old.log";
const ROTATE_AT: u64 = 256 * 1024;
static LOG: DiagnosticLog = DiagnosticLog::new();

/// Where capture.log is written, for Settings to show.
pub fn log_path() -> Option<PathBuf> {
    Some(log_dir()?.join(CURRENT))
}

// Unit tests construct and save app state without touching the desktop user's logs.
fn app_log_dir() -> Option<PathBuf> {
    if cfg!(test) { None } else { log_dir() }
}

/// Turns the diagnostic log on or off. Turning it off deletes both log files.
pub fn set_trace(on: bool) {
    LOG.set_enabled(on, app_log_dir);
}

/// Appends a diagnostic line when the user has enabled logging.
pub fn trace(message: fmt::Arguments) {
    LOG.append(message, app_log_dir);
}

#[cfg(target_os = "macos")]
pub(super) fn trace_enabled() -> bool {
    *lock(&LOG.enabled)
}

struct DiagnosticLog {
    // One lock covers the consent check, rotation, appending and deletion. A writer that has
    // already passed the check finishes before disable deletes its output.
    enabled: Mutex<bool>,
}

impl DiagnosticLog {
    const fn new() -> Self {
        Self { enabled: Mutex::new(false) }
    }

    fn set_enabled(&self, on: bool, dir: impl FnOnce() -> Option<PathBuf>) {
        let mut enabled = lock(&self.enabled);
        *enabled = on;
        if !on && let Some(dir) = dir() {
            let _ = std::fs::remove_file(dir.join(CURRENT));
            let _ = std::fs::remove_file(dir.join(ROTATED));
        }
    }

    fn append(&self, message: fmt::Arguments, dir: impl FnOnce() -> Option<PathBuf>) {
        let enabled = lock(&self.enabled);
        if !*enabled {
            return;
        }
        let Some(dir) = dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(CURRENT);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > ROTATE_AT) {
            let _ = std::fs::rename(&path, dir.join(ROTATED));
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let seconds = now.as_secs();
        let millis = now.subsec_millis();
        let line = if cfg!(target_os = "macos") {
            format!("{seconds} {message}\n")
        } else if cfg!(windows) {
            format!("{seconds}.{millis:03} {message}\r\n")
        } else {
            format!("{seconds}.{millis:03} {message}\n")
        };
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::TryLockError;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use supereditr_core::config::Config;

    struct HeldLine {
        started: Sender<()>,
        finish: Receiver<()>,
    }

    impl fmt::Display for HeldLine {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.started.send(()).unwrap();
            self.finish.recv().unwrap();
            f.write_str("review in progress")
        }
    }

    #[test]
    fn disabling_during_an_append_removes_both_logs_and_keeps_them_off_after_restart() {
        let dir = std::env::temp_dir().join(format!("supereditr-diagnostic-log-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let config_path = dir.join("settings.json");
        let current = dir.join(CURRENT);
        let rotated = dir.join(ROTATED);
        let log_dir = || Some(dir.clone());
        let log = DiagnosticLog::new();

        let mut config = Config::load(&config_path);
        log.set_enabled(config.diagnostic_log, log_dir);
        log.append(format_args!("review before opt-in"), log_dir);
        assert!(!current.exists() && !rotated.exists());

        config.diagnostic_log = true;
        config.save(&config_path).unwrap();
        log.set_enabled(config.diagnostic_log, log_dir);
        log.append(format_args!("review after opt-in"), log_dir);
        assert!(std::fs::read_to_string(&current).unwrap().contains("review after opt-in"));
        std::fs::OpenOptions::new().write(true).open(&current).unwrap().set_len(ROTATE_AT + 1).unwrap();

        let (started, writing) = channel();
        let (finish, held) = channel();
        let (disabling, disable_started) = channel();
        let line = HeldLine { started, finish: held };
        let log = &log;
        config.diagnostic_log = false;
        config.save(&config_path).unwrap();
        std::thread::scope(|threads| {
            threads.spawn(move || log.append(format_args!("{line}"), log_dir));
            writing.recv().unwrap();
            let locked = matches!(log.enabled.try_lock(), Err(TryLockError::WouldBlock));
            let did_rotate = rotated.is_file();
            threads.spawn(move || {
                disabling.send(()).unwrap();
                log.set_enabled(config.diagnostic_log, log_dir);
            });
            disable_started.recv().unwrap();
            finish.send(()).unwrap();
            assert!(locked && did_rotate);
        });

        log.append(format_args!("review after opt-out"), log_dir);
        assert!(!current.exists() && !rotated.exists());

        let restarted = DiagnosticLog::new();
        restarted.set_enabled(Config::load(&config_path).diagnostic_log, log_dir);
        restarted.append(format_args!("review after restart"), log_dir);
        assert!(!current.exists() && !rotated.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
