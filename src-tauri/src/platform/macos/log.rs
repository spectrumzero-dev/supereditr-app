//! capture.log, the diagnostic log of selection reads and writes.

pub(in crate::platform) fn log_dir() -> Option<std::path::PathBuf> {
    Some(std::path::Path::new(&std::env::var_os("HOME")?).join("Library/Logs/SuperEditr"))
}
