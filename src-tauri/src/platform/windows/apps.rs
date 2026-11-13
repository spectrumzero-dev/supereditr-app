//! What SuperEditr may do in each app, by program file name (`notepad.exe`), as data.
//!
//! Any app takes a fix unless its profile says otherwise: the replace chain checks the field,
//! selection and text before pasting and verifies the result after, so an app that behaves
//! differently ends in an "uncertain" outcome rather than a silent wrong edit. Measured runs in
//! Notepad, Chrome, Edge, Firefox, VS Code (with `editor.accessibilitySupport` on), Discord and the
//! new Outlook took a checked paste and undid it with one Ctrl+Z. Terminals, password managers and
//! the apps in `NO_PASTE` stay copy-only.

use supereditr_core::config::is_terminal;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Profile {
    /// A checked paste may be sent here.
    pub pastes: bool,
    /// Copy-only, and the guarded copy uses Ctrl+Insert (Ctrl+C would interrupt a command).
    pub terminal: bool,
    /// Nothing is read here, not even on a shortcut press.
    pub password_manager: bool,
    /// A Monaco editor, which shows accessibility only part of a long selection and has a terminal
    /// panel inside it. Its selection is checked against a copy.
    pub code_editor: bool,
    /// Chromium answers the first accessibility queries with its window or a `View` pane; the
    /// focused field comes a few queries later. Electron apps too.
    pub chromium: bool,
    /// Firefox's own UI Automation misses a field focused before SuperEditr first asked.
    pub gecko: bool,
}

/// Apps that stay copy-only even when a field reads cleanly: remote-desktop and virtual-machine
/// windows, whose Ctrl+V goes to another computer, and LibreOffice, whose UI Automation loses the
/// selection after arrow-key moves.
const NO_PASTE: &[&str] = &[
    "mstsc.exe",
    "msrdc.exe",
    "vmconnect.exe",
    "VirtualBoxVM.exe",
    "vmware.exe",
    "vncviewer.exe",
    "parsecd.exe",
    "AnyDesk.exe",
    "TeamViewer.exe",
    "rustdesk.exe",
    "soffice.bin",
];

const PASSWORD_MANAGERS: &[&str] = &["1Password.exe", "Bitwarden.exe", "KeePassXC.exe", "KeePass.exe", "NordPass.exe"];

const CODE_EDITORS: &[&str] = &["Code.exe", "Code - Insiders.exe", "VSCodium.exe", "Cursor.exe", "Windsurf.exe"];

/// Chromium, Electron and WebView2 apps.
const CHROMIUM: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "brave.exe",
    "vivaldi.exe",
    "opera.exe",
    "Code.exe",
    "Discord.exe",
    "Slack.exe",
    "olk.exe",
    "ms-teams.exe",
    "msedgewebview2.exe",
    "Obsidian.exe",
    "Notion.exe",
    "claude.exe",
];

/// Window classes of terminals, whatever program hosts them.
const TERMINAL_WINDOWS: &[&str] = &["ConsoleWindowClass", "CASCADIA_HOSTING_WINDOW_CLASS", "PseudoConsoleWindow"];
/// Element classes of terminals inside other apps: Windows Terminal's control, and VS Code's
/// terminal panel.
const TERMINAL_ELEMENTS: &[&str] = &["TermControl", "xterm-helper-textarea"];

fn listed(list: &[&str], image: &str) -> bool {
    list.iter().any(|name| name.eq_ignore_ascii_case(image))
}

/// The profile of the program `image` whose window has class `window_class`.
pub fn profile(image: &str, window_class: &str) -> Profile {
    let terminal = is_terminal(image) || listed(TERMINAL_WINDOWS, window_class);
    let password_manager = listed(PASSWORD_MANAGERS, image);
    Profile {
        pastes: !listed(NO_PASTE, image) && !terminal && !password_manager,
        terminal,
        password_manager,
        code_editor: listed(CODE_EDITORS, image),
        chromium: listed(CHROMIUM, image),
        gecko: image.eq_ignore_ascii_case("firefox.exe"),
    }
}

/// Whether a focused element is a terminal, inside whatever app.
pub fn terminal_element(class: &str) -> bool {
    listed(TERMINAL_ELEMENTS, class)
}

/// Whether a focused element is a field of mail recipients, such as Outlook's To, Cc and Bcc. They
/// turn typed text into a recipient when focus leaves them, so a fix there would change who the
/// mail goes to.
pub fn recipient_field(control_type: i32, class: &str, name: &str) -> bool {
    const GROUP: i32 = 50026;
    let name = name.trim().trim_end_matches(':');
    // The new Outlook lists EditorClass among a dozen generated class names.
    control_type == GROUP
        && class.split_whitespace().any(|c| c == "EditorClass")
        && ["To", "Cc", "Bcc"].iter().any(|field| field.eq_ignore_ascii_case(name))
}

/// A readable name for a program, for messages: "notepad.exe" becomes "Notepad", "msedge.exe"
/// "Microsoft Edge".
pub fn display_name(image: &str) -> String {
    const KNOWN: &[(&str, &str)] = &[
        ("msedge.exe", "Microsoft Edge"),
        ("chrome.exe", "Google Chrome"),
        ("firefox.exe", "Firefox"),
        ("Code.exe", "Visual Studio Code"),
        ("olk.exe", "Outlook"),
        ("ms-teams.exe", "Microsoft Teams"),
        ("WindowsTerminal.exe", "Terminal"),
        ("soffice.bin", "LibreOffice"),
        ("soffice.exe", "LibreOffice"),
        ("WINWORD.EXE", "Word"),
        ("cmd.exe", "Command Prompt"),
        ("powershell.exe", "PowerShell"),
        ("pwsh.exe", "PowerShell"),
        ("claude.exe", "Claude"),
    ];
    if let Some((_, name)) = KNOWN.iter().find(|(file, _)| file.eq_ignore_ascii_case(image)) {
        return (*name).to_string();
    }
    let stem = image.rsplit_once('.').map_or(image, |(stem, _)| stem);
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "this app".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_take_pastes_unless_listed_and_names_ignore_case() {
        for app in [
            "notepad.exe",
            "chrome.exe",
            "Code.exe",
            "olk.exe",
            "claude.exe",
            "WINWORD.EXE",
            "ms-teams.exe",
            "QuillPoint.exe",
        ] {
            assert!(profile(app, "").pastes, "{app}");
        }
        for app in ["mstsc.exe", "MSTSC.EXE", "vmconnect.exe", "AnyDesk.exe", "soffice.bin"] {
            assert!(!profile(app, "").pastes, "{app} stays copy-only");
        }
    }

    #[test]
    fn terminals_are_known_by_program_or_window() {
        assert!(profile("WindowsTerminal.exe", "").terminal);
        assert!(profile("QuillShell.exe", "ConsoleWindowClass").terminal, "a console window, whoever hosts it");
        assert!(!profile("WindowsTerminal.exe", "").pastes);
        // The Claude app and Claude Code share claude.exe; Claude Code in a console stays copy-only.
        assert!(!profile("claude.exe", "ConsoleWindowClass").pastes, "Claude Code in a console");
        assert!(terminal_element("xterm-helper-textarea"), "VS Code's terminal panel");
        assert!(terminal_element("TermControl"));
        assert!(!terminal_element("native-edit-context"), "VS Code's editor");
    }

    #[test]
    fn password_managers_are_never_written() {
        let keepass = profile("KeePassXC.exe", "");
        assert!(keepass.password_manager && !keepass.pastes);
    }

    #[test]
    fn recipient_fields_are_recognised_by_name() {
        assert!(recipient_field(50026, "EditorClass", "To"));
        assert!(recipient_field(50026, "EditorClass", "Cc:"));
        assert!(
            recipient_field(50026, "___h7sz9k0 fwg0e2s EditorClass customScrollBar", "To"),
            "the new Outlook's classes"
        );
        assert!(!recipient_field(50026, "EditorClass", "Message body"));
        assert!(!recipient_field(50004, "EditorClass", "To"), "an edit control named To is an ordinary field");
    }

    #[test]
    fn programs_get_readable_names() {
        assert_eq!(display_name("notepad.exe"), "Notepad");
        assert_eq!(display_name("MSEDGE.EXE"), "Microsoft Edge");
        assert_eq!(display_name("quillpoint.exe"), "Quillpoint");
        assert_eq!(display_name(""), "this app");
    }
}
