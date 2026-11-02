//! The Linux setup page, and Set up Omarchy with its uninstall.
//!
//! Set up Omarchy changes only files in the user's own config folder, and says which before it
//! does: it adds `--force-renderer-accessibility` to the Chromium and Electron flag files, so their
//! pages expose text, and turns on opening at login. Each flag line it adds follows a comment that
//! marks it as SuperEditr's, which is how `supereditr uninstall-omarchy` finds exactly what to take
//! out. Omarchy's own files (`/usr/share/omarchy`) and the Hyprland config (`~/.config/hypr/`) are
//! never touched: SuperEditr's binds and window rule exist only at runtime.

use super::desktop::SetupCheck;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Makes Chromium and Electron expose web page text to accessibility.
pub const FLAG: &str = "--force-renderer-accessibility";
/// The flag files Set up Omarchy adds the flag to, in the config folder. Arch's `chromium` launcher
/// and `electron*` wrappers read them, and both skip lines starting with `#`.
pub const FLAG_FILES: &[&str] = &["chromium-flags.conf", "electron-flags.conf"];
/// The autostart entry, in the config folder. The autostart plugin writes it.
pub const AUTOSTART: &str = "autostart/supereditr.desktop";

/// Every comment SuperEditr writes starts so; the flag line follows it.
const MARK: &str = "# SuperEditr added the next line so it can read web pages and Electron apps.";
const REMOVE_HINT: &str = " `supereditr uninstall-omarchy` removes both.";
/// Added to the comment when SuperEditr created the file, so the uninstall removes it.
const CREATED: &str = " It created this file.";
/// Added when the file didn't end in a line break, so the uninstall takes that out too.
const JOINED: &str = " It also added the line break before this comment.";

/// The user's config folder: `$XDG_CONFIG_HOME`, or `~/.config`.
pub fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

/// A path for display: `~/.config/…` in place of the home folder.
pub fn tilde(path: &Path) -> String {
    match std::env::var_os("HOME").map(PathBuf::from) {
        Some(home) if path.starts_with(&home) => {
            format!("~/{}", path.strip_prefix(&home).unwrap_or(path).display())
        }
        _ => path.display().to_string(),
    }
}

/// Whether a flag file's lines already pass the flag, from SuperEditr or not.
fn has_flag(text: &str) -> bool {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .any(|line| line.split_whitespace().any(|w| w == FLAG))
}

/// The text of a flag file with SuperEditr's lines added, or `None` when the flag is already there.
/// `existing` is `None` for a missing file.
fn with_flag(existing: Option<&str>) -> Option<String> {
    match existing {
        Some(text) if has_flag(text) => None,
        None => Some(format!("{MARK}{REMOVE_HINT}{CREATED}\n{FLAG}\n")),
        Some(text) if !text.is_empty() && !text.ends_with('\n') => {
            Some(format!("{text}\n{MARK}{REMOVE_HINT}{JOINED}\n{FLAG}\n"))
        }
        Some(text) => Some(format!("{text}{MARK}{REMOVE_HINT}\n{FLAG}\n")),
    }
}

/// What taking SuperEditr's lines out of a flag file leaves.
#[derive(Debug, PartialEq, Eq)]
enum Without {
    /// No SuperEditr lines: leave the file alone.
    Unchanged,
    /// Write this back.
    Text(String),
    /// SuperEditr created the file and nothing else is in it: delete it.
    Delete,
}

fn without_flag(text: &str) -> Without {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut kept = String::with_capacity(text.len());
    let mut created = false;
    let mut found = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let next_is_flag = lines.get(i + 1).is_some_and(|next| next.trim_end_matches(['\n', '\r']) == FLAG);
        if line.starts_with(MARK) && next_is_flag {
            found = true;
            created |= line.contains(CREATED.trim());
            if line.contains(JOINED.trim()) && kept.ends_with('\n') {
                kept.pop();
            }
            i += 2;
            continue;
        }
        kept.push_str(line);
        i += 1;
    }
    match (found, created) {
        (false, _) => Without::Unchanged,
        (true, true) if kept.trim().is_empty() => Without::Delete,
        (true, _) => Without::Text(kept),
    }
}

/// One change Set up Omarchy makes, as the page lists it before making it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    /// The file, as `~/.config/…`.
    pub path: String,
    pub what: String,
    /// False when it's already in place.
    pub needed: bool,
}

/// The flag-file changes, for the files under `config`.
pub fn flag_changes(config: &Path) -> Vec<Change> {
    FLAG_FILES
        .iter()
        .map(|name| {
            let path = config.join(name);
            let existing = std::fs::read_to_string(&path).ok();
            let app = if name.starts_with("chromium") { "Chromium" } else { "Electron apps" };
            let what = match (&existing, with_flag(existing.as_deref())) {
                (_, None) => format!("{app} already expose web page text ({FLAG} is set)."),
                (None, Some(_)) => format!("Create it with {FLAG}, so {app} expose web page text."),
                (Some(_), Some(_)) => format!("Add {FLAG}, with a comment marking it as SuperEditr’s."),
            };
            Change { needed: with_flag(existing.as_deref()).is_some(), path: tilde(&path), what }
        })
        .collect()
}

/// Adds the flag to each flag file under `config` that lacks it. Returns what it did.
pub fn add_flags(config: &Path) -> std::io::Result<Vec<String>> {
    let mut done = Vec::new();
    for name in FLAG_FILES {
        let path = config.join(name);
        let existing = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        if let Some(text) = with_flag(existing.as_deref()) {
            std::fs::create_dir_all(config)?;
            write_whole(&path, &text)?;
            done.push(format!("Added {FLAG} to {}", tilde(&path)));
        }
    }
    Ok(done)
}

/// Takes SuperEditr's lines out of the flag files under `config`, and deletes a file SuperEditr
/// created once nothing else is in it. Returns what it did.
pub fn remove_flags(config: &Path) -> std::io::Result<Vec<String>> {
    let mut done = Vec::new();
    for name in FLAG_FILES {
        let path = config.join(name);
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        match without_flag(&text) {
            Without::Unchanged => {}
            Without::Text(text) => {
                write_whole(&path, &text)?;
                done.push(format!("Removed SuperEditr’s {FLAG} lines from {}", tilde(&path)));
            }
            Without::Delete => {
                std::fs::remove_file(&path)?;
                done.push(format!("Deleted {}, which SuperEditr created", tilde(&path)));
            }
        }
    }
    Ok(done)
}

/// Writes a file in one step, keeping its permissions, so a crash can't leave half of it.
fn write_whole(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("conf.supereditr-tmp");
    std::fs::write(&tmp, text)?;
    if let Ok(meta) = std::fs::metadata(path) {
        std::fs::set_permissions(&tmp, meta.permissions())?;
    }
    std::fs::rename(tmp, path)
}

/// Removes the autostart entry. Returns what it did.
pub fn remove_autostart(config: &Path) -> std::io::Result<Option<String>> {
    let path = config.join(AUTOSTART);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(Some(format!("Deleted {}, so SuperEditr no longer opens at login", tilde(&path)))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Asks systemd to read the autostart entries again. Its user manager turns them into units only
/// when it starts or reloads, and it can outlive a login (an open SSH session, lingering), so an
/// entry added or removed meanwhile would otherwise be missed at the next login.
pub fn reload_autostart() {
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Whether the autostart entry is there and starts a SuperEditr that exists.
pub fn autostart_check(config: &Path) -> SetupCheck {
    let path = config.join(AUTOSTART);
    let exec = std::fs::read_to_string(&path).ok().and_then(|text| {
        text.lines()
            .find_map(|line| line.strip_prefix("Exec="))
            .map(|exec| exec.split_whitespace().next().unwrap_or("").to_string())
    });
    let (ok, detail) = match exec {
        None => (false, "Off. Set up Omarchy turns it on.".to_string()),
        Some(exec) if Path::new(&exec).is_file() => (true, format!("On ({})", tilde(&path))),
        Some(exec) => (false, format!("{} starts {exec}, which doesn’t exist", tilde(&path))),
    };
    SetupCheck { name: "Open at login".into(), ok, detail }
}

/// Whether Set up Omarchy's changes are all in place: the flag lines and opening at login. Settings
/// then stops opening on its own at launch, which would otherwise be at every login.
pub fn complete(config: &Path) -> bool {
    flags_check(config).ok && autostart_check(config).ok
}

/// The flag files' line on the setup page. Also flags per-version Electron files
/// (`electron43-flags.conf`), which Arch's wrapper reads in place of `electron-flags.conf`.
pub fn flags_check(config: &Path) -> SetupCheck {
    let missing: Vec<String> = FLAG_FILES
        .iter()
        .filter(|name| !std::fs::read_to_string(config.join(name)).is_ok_and(|text| has_flag(&text)))
        .map(|name| tilde(&config.join(name)))
        .collect();
    let overrides: Vec<String> = std::fs::read_dir(config)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            name.starts_with("electron")
                && name.ends_with("-flags.conf")
                && name != "electron-flags.conf"
                && !std::fs::read_to_string(config.join(name)).is_ok_and(|text| has_flag(&text))
        })
        .collect();
    let mut detail = if missing.is_empty() {
        format!("{FLAG} is set for Chromium and Electron apps")
    } else {
        format!("{FLAG} isn’t in {}. Set up Omarchy adds it.", missing.join(" or "))
    };
    if !overrides.is_empty() {
        detail.push_str(&format!(
            " Electron apps that use {} read that file instead: add {FLAG} there yourself.",
            overrides.join(", ")
        ));
    }
    SetupCheck { name: "Web pages and Electron apps".into(), ok: missing.is_empty() && overrides.is_empty(), detail }
}

/// Whether a keyring in `data` (`~/.local/share`) is stored without a password. GNOME Keyring
/// writes an unlocked keyring as plain text starting `[keyring]`, and a locked one in a binary
/// format. Omarchy's default keyring has no password.
pub fn plain_keyrings(data: &Path) -> Vec<String> {
    std::fs::read_dir(data.join("keyrings"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "keyring"))
        .filter(|entry| {
            let mut head = [0u8; 9];
            std::fs::File::open(entry.path())
                .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut head))
                .is_ok_and(|()| &head == b"[keyring]")
        })
        .map(|entry| entry.path().file_stem().unwrap_or_default().to_string_lossy().into_owned())
        .collect()
}

/// A Chromium or Electron program: they keep `resources.pak` next to the executable.
pub fn chromium_based(pid: i32) -> bool {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("resources.pak").is_file()))
        .unwrap_or(false)
}

/// Whether a process was started with the flag.
pub fn started_with_flag(pid: i32) -> bool {
    std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|cmdline| has_flag_arg(&cmdline))
}

/// Whether a `/proc/<pid>/cmdline` passes the flag. Chromium rewrites its own as one string with
/// spaces between the arguments, in place of the usual NUL bytes.
fn has_flag_arg(cmdline: &[u8]) -> bool {
    cmdline.split(|b| *b == 0 || b.is_ascii_whitespace()).any(|arg| arg == FLAG.as_bytes())
}

/// `supereditr uninstall-omarchy [--purge]`: takes out what Set up Omarchy added. With `--purge`,
/// also deletes SuperEditr's settings, log and saved API keys. Returns the exit code.
pub fn uninstall_command(args: &[String]) -> i32 {
    let purge = args.iter().any(|arg| arg == "--purge");
    if let Some(unknown) = args.iter().find(|arg| *arg != "--purge") {
        eprintln!(
            "supereditr uninstall-omarchy: unknown option {unknown}. Usage: supereditr uninstall-omarchy [--purge]"
        );
        return 2;
    }
    let Some(config) = config_home() else {
        eprintln!("supereditr uninstall-omarchy: HOME isn’t set");
        return 1;
    };
    let mut failed = false;
    let mut report = |result: std::io::Result<Vec<String>>| match result {
        Ok(done) => done.iter().for_each(|line| println!("{line}")),
        Err(e) => {
            eprintln!("supereditr uninstall-omarchy: {e}");
            failed = true;
        }
    };
    report(remove_flags(&config));
    report(remove_autostart(&config).map(|done| done.into_iter().collect()));
    reload_autostart();
    if purge {
        report(purge_data(&config));
    } else {
        println!(
            "Kept SuperEditr’s settings and saved API keys. `supereditr uninstall-omarchy --purge` deletes them too."
        );
    }
    i32::from(failed)
}

/// Deletes SuperEditr's settings folders, its log and the API keys its settings name.
fn purge_data(config: &Path) -> std::io::Result<Vec<String>> {
    let mut done = Vec::new();
    for id in [crate::IDENTIFIER, crate::OLD_IDENTIFIER] {
        let dir = config.join(id);
        let settings = dir.join("settings.json");
        let providers: Vec<String> = std::fs::read_to_string(&settings)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|json| json["providers"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|p| p["id"].as_str().map(str::to_string))
            .collect();
        for provider in &providers {
            crate::secrets::delete(provider);
        }
        if !providers.is_empty() {
            done.push(format!("Deleted the saved API keys for {}", providers.join(", ")));
        }
        if dir.is_dir() {
            std::fs::remove_dir_all(&dir)?;
            done.push(format!("Deleted {}", tilde(&dir)));
        }
    }
    if let Some(log) = super::log_path().and_then(|log| log.parent().map(Path::to_path_buf))
        && log.is_dir()
    {
        std::fs::remove_dir_all(&log)?;
        done.push(format!("Deleted {}", tilde(&log)));
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OMARCHY: &str = "--ozone-platform=wayland\n--ozone-platform-hint=wayland\n--password-store=gnome-libsecret\n";

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("supereditr-setup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_flag_goes_after_a_comment_and_comes_out_exactly() {
        let added = with_flag(Some(OMARCHY)).unwrap();
        assert!(added.starts_with(OMARCHY));
        assert!(added.ends_with(&format!("{FLAG}\n")));
        assert!(has_flag(&added));
        assert_eq!(without_flag(&added), Without::Text(OMARCHY.into()));
    }

    #[test]
    fn a_flag_already_there_is_left_alone() {
        let theirs = format!("{OMARCHY}{FLAG}\n");
        assert_eq!(with_flag(Some(&theirs)), None);
        assert_eq!(without_flag(&theirs), Without::Unchanged, "the user's own line stays");
        assert_eq!(with_flag(Some("--a --force-renderer-accessibility\n")), None);
        assert!(with_flag(Some("# --force-renderer-accessibility\n")).is_some(), "a comment doesn't count");
    }

    #[test]
    fn a_file_supereditr_created_is_deleted_when_only_its_lines_are_left() {
        let created = with_flag(None).unwrap();
        assert!(has_flag(&created));
        assert_eq!(without_flag(&created), Without::Delete);
        // The user added a line of their own since: the file stays, with it.
        assert_eq!(without_flag(&format!("{created}--enable-foo\n")), Without::Text("--enable-foo\n".into()));
    }

    #[test]
    fn a_file_without_a_final_line_break_gets_it_back_as_it_was() {
        let theirs = "--ozone-platform=wayland";
        let added = with_flag(Some(theirs)).unwrap();
        assert_eq!(without_flag(&added), Without::Text(theirs.into()));
        assert_eq!(with_flag(Some("")).map(|text| without_flag(&text)), Some(Without::Text(String::new())));
    }

    #[test]
    fn a_marker_without_its_flag_isnt_taken_for_supereditrs() {
        let edited = format!("{OMARCHY}{MARK}{REMOVE_HINT}\n--something-else\n");
        assert_eq!(without_flag(&edited), Without::Unchanged);
    }

    #[test]
    fn setting_up_and_removing_leaves_the_folder_as_it_was() {
        let config = folder("roundtrip");
        std::fs::write(config.join("chromium-flags.conf"), OMARCHY).unwrap();
        let changes = flag_changes(&config);
        assert_eq!(changes.len(), 2);
        assert!(changes.iter().all(|change| change.needed));

        let done = add_flags(&config).unwrap();
        assert_eq!(done.len(), 2);
        assert!(flags_check(&config).ok);
        assert!(flag_changes(&config).iter().all(|change| !change.needed));
        assert!(add_flags(&config).unwrap().is_empty(), "a second run changes nothing");

        let done = remove_flags(&config).unwrap();
        assert_eq!(done.len(), 2);
        assert_eq!(std::fs::read_to_string(config.join("chromium-flags.conf")).unwrap(), OMARCHY);
        assert!(!config.join("electron-flags.conf").exists());
        assert!(remove_flags(&config).unwrap().is_empty());
        std::fs::remove_dir_all(config).unwrap();
    }

    #[test]
    fn a_per_version_electron_file_is_flagged() {
        let config = folder("electron-version");
        add_flags(&config).unwrap();
        assert!(flags_check(&config).ok);
        std::fs::write(config.join("electron43-flags.conf"), "--ozone-platform=wayland\n").unwrap();
        let check = flags_check(&config);
        assert!(!check.ok);
        assert!(check.detail.contains("electron43-flags.conf"), "{}", check.detail);
        std::fs::remove_dir_all(config).unwrap();
    }

    #[test]
    fn the_autostart_entry_is_checked_and_removed() {
        let config = folder("autostart");
        assert!(!autostart_check(&config).ok);
        std::fs::create_dir_all(config.join("autostart")).unwrap();
        let entry = config.join(AUTOSTART);
        std::fs::write(&entry, "[Desktop Entry]\nType=Application\nExec=/bin/sh\n").unwrap();
        assert!(autostart_check(&config).ok);
        std::fs::write(&entry, "[Desktop Entry]\nExec=/nowhere/supereditr\n").unwrap();
        assert!(!autostart_check(&config).ok);
        assert!(remove_autostart(&config).unwrap().is_some());
        assert!(!entry.exists());
        assert!(remove_autostart(&config).unwrap().is_none());
        std::fs::remove_dir_all(config).unwrap();
    }

    #[test]
    fn setup_is_complete_with_both_the_flags_and_opening_at_login() {
        let config = folder("complete");
        assert!(!complete(&config));
        add_flags(&config).unwrap();
        assert!(!complete(&config), "the flags alone");
        std::fs::create_dir_all(config.join("autostart")).unwrap();
        std::fs::write(config.join(AUTOSTART), "[Desktop Entry]\nType=Application\nExec=/bin/sh\n").unwrap();
        assert!(complete(&config));
        remove_flags(&config).unwrap();
        assert!(!complete(&config), "opening at login alone");
        std::fs::remove_dir_all(config).unwrap();
    }

    #[test]
    fn a_keyring_without_a_password_is_found() {
        let data = folder("keyrings");
        std::fs::create_dir_all(data.join("keyrings")).unwrap();
        std::fs::write(data.join("keyrings/Default_keyring.keyring"), "[keyring]\ndisplay-name=Default keyring\n")
            .unwrap();
        std::fs::write(data.join("keyrings/login.keyring"), b"GnomeKeyring\n\r\0\n\x01\x00").unwrap();
        std::fs::write(data.join("keyrings/default"), "Default_keyring").unwrap();
        assert_eq!(plain_keyrings(&data), vec!["Default_keyring".to_string()]);
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn the_flag_is_found_in_either_kind_of_command_line() {
        assert!(has_flag_arg(b"/usr/lib/electron43/electron\0--force-renderer-accessibility\0app.asar\0"));
        // Chromium's own, as /proc shows it once Chromium has rewritten it.
        assert!(has_flag_arg(b"/usr/lib/chromium/chromium --ozone-platform=wayland --force-renderer-accessibility\0"));
        assert!(!has_flag_arg(b"/usr/lib/chromium/chromium --ozone-platform=wayland\0"));
        assert!(!has_flag_arg(b"/usr/lib/chromium/chromium --force-renderer-accessibility-extra\0"));
    }

    #[test]
    fn the_uninstall_command_refuses_unknown_options() {
        assert_eq!(uninstall_command(&["--purg".into()]), 2);
    }
}
