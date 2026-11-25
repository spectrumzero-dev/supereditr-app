//! capture.log, in `$XDG_STATE_HOME/supereditr/` (`~/.local/state/supereditr/` by default).

use std::path::{Path, PathBuf};
/// The XDG state directory: `$XDG_STATE_HOME` when it's an absolute path, as the XDG Base Directory
/// specification requires, otherwise `~/.local/state`.
fn state_dir(xdg_state_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    match xdg_state_home.filter(|dir| Path::new(dir).is_absolute()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => Some(Path::new(home.filter(|home| !home.is_empty())?).join(".local/state")),
    }
}

pub(in crate::platform) fn log_dir() -> Option<PathBuf> {
    let xdg = std::env::var("XDG_STATE_HOME").ok();
    let home = std::env::var("HOME").ok();
    Some(state_dir(xdg.as_deref(), home.as_deref())?.join("supereditr"))
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
