//! What SuperEditr may do in each app, by Wayland app ID (Hyprland's window class).
//!
//! An app SuperEditr can read through accessibility gets its corrections by paste. Everything else
//! is copy-only: the suggestion goes to the clipboard for the user to paste. Terminals are always
//! copy-only, because a paste there can run a command.

use supereditr_core::config::is_terminal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Write {
    /// Corrections are pasted over the selection, once accessibility confirms the field.
    Paste,
    /// Nothing is written; the suggestion goes to the clipboard.
    CopyOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    pub write: Write,
    /// The guarded copy uses Ctrl+Insert, which terminals take as copy (Ctrl+C interrupts).
    pub terminal: bool,
    /// Nothing is ever read here, not even on a shortcut press.
    pub password_manager: bool,
}

const PASSWORD_MANAGERS: &[&str] =
    &["1Password", "Bitwarden", "org.keepassxc.KeePassXC", "org.gnome.seahorse.Application"];

/// Apps whose text accessibility exposes only in part. Obsidian's editor shows one placeholder
/// character per line, so its selection is read by a guarded copy.
const COPY_ONLY: &[&str] = &["md.obsidian.Obsidian", "obsidian"];

/// The profile of the app with this ID. `tags` are the window's Hyprland tags: Omarchy tags every
/// terminal it knows `terminal`, which catches terminals missing from SuperEditr's own list.
pub fn profile(app_id: &str, tags: &[String]) -> Profile {
    let listed = |list: &[&str]| list.iter().any(|id| id.eq_ignore_ascii_case(app_id));
    let terminal = is_terminal(app_id) || tags.iter().any(|tag| tag.starts_with("terminal"));
    let password_manager = listed(PASSWORD_MANAGERS);
    let write = if terminal || password_manager || listed(COPY_ONLY) { Write::CopyOnly } else { Write::Paste };
    Profile { write, terminal, password_manager }
}

/// A readable name for an app ID, for messages: "org.gnome.TextEditor" becomes "TextEditor",
/// "chromium" becomes "Chromium".
pub fn display_name(app_id: &str) -> String {
    let last = app_id.rsplit('.').next().unwrap_or(app_id);
    let mut chars = last.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "this app".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_apps_take_pastes() {
        for app in ["org.gnome.TextEditor", "chromium", "firefox", "featherpad"] {
            assert_eq!(profile(app, &[]).write, Write::Paste, "{app}");
        }
    }

    #[test]
    fn terminals_password_managers_and_partly_readable_apps_are_copy_only() {
        let foot = profile("foot", &[]);
        assert!(foot.terminal);
        assert_eq!(foot.write, Write::CopyOnly);
        assert_eq!(profile("md.obsidian.Obsidian", &[]).write, Write::CopyOnly);
        let keepass = profile("org.keepassxc.KeePassXC", &[]);
        assert!(keepass.password_manager);
        assert_eq!(keepass.write, Write::CopyOnly);
    }

    #[test]
    fn omarchys_terminal_tag_marks_a_terminal_supereditr_doesnt_know() {
        let tagged = profile("com.quillpoint.Term", &["terminal*".into()]);
        assert!(tagged.terminal);
        assert_eq!(tagged.write, Write::CopyOnly);
        assert!(!profile("com.quillpoint.Term", &["chat".into()]).terminal);
    }

    #[test]
    fn app_names_are_the_ids_last_part() {
        assert_eq!(display_name("org.gnome.TextEditor"), "TextEditor");
        assert_eq!(display_name("chromium"), "Chromium");
        assert_eq!(display_name(""), "this app");
    }
}
