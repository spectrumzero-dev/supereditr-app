//! capture.log, in `%LOCALAPPDATA%\SuperEditr\Logs\`.

use std::path::{Path, PathBuf};
/// The log folder under `local_app_data`, the per-user, per-machine app data folder.
fn log_dir_in(local_app_data: Option<&Path>) -> Option<PathBuf> {
    Some(local_app_data.filter(|dir| dir.is_absolute())?.join(r"SuperEditr\Logs"))
}

pub(in crate::platform) fn log_dir() -> Option<PathBuf> {
    log_dir_in(std::env::var_os("LOCALAPPDATA").as_deref().map(Path::new))
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
