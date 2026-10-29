//! When SuperEditr first ran on this Mac, and which version that was, kept in `install.json` next to
//! settings.json. It is written once and never changed, so a later version can tell who used the app
//! before a feature existed. Resetting settings leaves it alone.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Install {
    /// The version that wrote this record, e.g. "0.1.0".
    pub first_version: String,
    /// When that was, in seconds since 1970 (UTC).
    pub first_run: u64,
    /// Settings already existed then, so an earlier build without this record ran before it.
    pub had_settings: bool,
}

impl Install {
    /// Reads the record at `path`, first writing one for this run if there is none. A record that
    /// exists but can't be read is left as it is and reported as an error.
    pub fn record(path: &Path, version: &str, had_settings: bool) -> std::io::Result<Install> {
        if path.exists() {
            let raw = std::fs::read_to_string(path)?;
            return serde_json::from_str(&raw).map_err(std::io::Error::other);
        }
        let install = Install {
            first_version: version.into(),
            first_run: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            had_settings,
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&install).map_err(std::io::Error::other)?)?;
        std::fs::rename(tmp, path)?;
        Ok(install)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_first_run() {
        let dir = std::env::temp_dir().join(format!("supereditr-install-{}", std::process::id()));
        let path = dir.join("install.json");
        let first = Install::record(&path, "0.1.0", true).unwrap();
        assert_eq!(first.first_version, "0.1.0");
        assert!(first.had_settings);
        assert!(first.first_run > 0);
        let later = Install::record(&path, "0.2.0", false).unwrap();
        assert_eq!(later, first, "a later version keeps the first record");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn leaves_an_unreadable_record_alone() {
        let dir = std::env::temp_dir().join(format!("supereditr-install-bad-{}", std::process::id()));
        let path = dir.join("install.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "not json").unwrap();
        assert!(Install::record(&path, "0.2.0", false).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
