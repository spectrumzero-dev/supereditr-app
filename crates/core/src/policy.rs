//! Rules every platform's integration applies the same way, written once over plain data: which
//! apps are password managers or Chromium browsers, whether a selection is still the one captured,
//! and which parts of a capture a review leaves as written. Each platform supplies what it can read.

/// The operating system whose identifiers an app is known by: a bundle ID on macOS, a Wayland app
/// ID (Hyprland's window class) on Linux, a program file name on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Linux,
    Windows,
}

impl Os {
    /// The system this build runs on; macOS for any system without an integration of its own, as
    /// the default settings are.
    #[cfg(windows)]
    pub const CURRENT: Os = Os::Windows;
    #[cfg(target_os = "linux")]
    pub const CURRENT: Os = Os::Linux;
    #[cfg(not(any(target_os = "linux", windows)))]
    pub const CURRENT: Os = Os::MacOs;

    /// Whether `id` names the app listed as `listed`. A macOS bundle ID matches by prefix, so a
    /// browser's beta and canary builds count as the browser; Linux app IDs and Windows file names
    /// match whole, ignoring case.
    fn matches(self, listed: &str, id: &str) -> bool {
        match self {
            Os::MacOs => id.starts_with(listed),
            Os::Linux | Os::Windows => listed.eq_ignore_ascii_case(id),
        }
    }
}

/// One app, by the identifier it has on each system it is known on.
#[derive(Debug, Clone, Copy)]
pub struct App {
    pub macos: Option<&'static str>,
    pub linux: Option<&'static str>,
    pub windows: Option<&'static str>,
}

impl App {
    fn id(&self, os: Os) -> Option<&'static str> {
        match os {
            Os::MacOs => self.macos,
            Os::Linux => self.linux,
            Os::Windows => self.windows,
        }
    }
}

const fn app(macos: Option<&'static str>, linux: Option<&'static str>, windows: Option<&'static str>) -> App {
    App { macos, linux, windows }
}

/// Password managers. On Linux and Windows nothing is ever read from one, not even on a shortcut
/// press; macOS relies on its default exclusions, which the user can turn off.
pub const PASSWORD_MANAGERS: &[App] = &[
    app(Some("com.1password.1password"), Some("1Password"), Some("1Password.exe")),
    // 1Password 7 from the Mac App Store.
    app(Some("com.agilebits.onepassword7"), None, None),
    app(Some("com.bitwarden.desktop"), Some("Bitwarden"), Some("Bitwarden.exe")),
    app(Some("org.keepassxc.keepassxc"), Some("org.keepassxc.KeePassXC"), Some("KeePassXC.exe")),
    app(None, None, Some("KeePass.exe")),
    app(Some("com.nordsec.nordpass"), Some("NordPass"), Some("NordPass.exe")),
    app(Some("com.apple.keychainaccess"), None, None),
    app(Some("com.apple.Passwords"), None, None),
    app(None, Some("org.gnome.seahorse.Application"), None),
];

/// Chromium browsers. Apps built on Chromium (Electron, WebView2) are found another way on each
/// system. Linux finds every Chromium app by its runtime, so it lists none here.
pub const CHROMIUM_BROWSERS: &[App] = &[
    app(Some("com.google.Chrome"), None, Some("chrome.exe")),
    app(Some("com.brave.Browser"), None, Some("brave.exe")),
    app(Some("com.microsoft.edgemac"), None, Some("msedge.exe")),
    app(Some("company.thebrowser"), None, None),
    app(Some("com.vivaldi.Vivaldi"), None, Some("vivaldi.exe")),
    app(Some("com.operasoftware.Opera"), None, Some("opera.exe")),
    app(Some("org.chromium.Chromium"), None, None),
];

fn listed(apps: &[App], os: Os, id: &str) -> bool {
    apps.iter().filter_map(|app| app.id(os)).any(|listed| os.matches(listed, id))
}

/// Whether the app `id` names on `os` is a password manager.
pub fn is_password_manager(os: Os, id: &str) -> bool {
    listed(PASSWORD_MANAGERS, os, id)
}

/// Whether the app `id` names on `os` is a Chromium browser.
pub fn is_chromium_browser(os: Os, id: &str) -> bool {
    listed(CHROMIUM_BROWSERS, os, id)
}

/// A selection as a platform reads it: its text, and the text on either side of it where the
/// platform reads that too.
#[derive(Debug)]
pub struct Selection<'a, T: ?Sized> {
    pub text: &'a T,
    pub context: Option<(&'a T, &'a T)>,
}

// Written out: a derive would ask `T: Clone`, which `str` and `[u16]` aren't.
impl<T: ?Sized> Clone for Selection<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for Selection<'_, T> {}

impl<'a, T: ?Sized> Selection<'a, T> {
    /// A selection read without the text around it.
    pub fn text(text: &'a T) -> Self {
        Selection { text, context: None }
    }

    /// A selection read with the text before and after it.
    pub fn with_context(text: &'a T, before: &'a T, after: &'a T) -> Self {
        Selection { text, context: Some((before, after)) }
    }
}

/// Whether the selection `now` is still the `captured` one, checked before anything replaces it.
/// `same_place` is the platform's own answer to whether it is the same field with the selection in
/// the same place, as far as it can tell. Then the text must be the same, and so must the text
/// around it: a read that has context only on one side is not the same.
pub fn selection_unchanged<T: PartialEq + ?Sized>(
    same_place: bool,
    captured: Selection<'_, T>,
    now: Selection<'_, T>,
) -> bool {
    same_place && captured.text == now.text && captured.context == now.context
}

/// The character ranges of a capture's text a review leaves as written, as `edits::plan` and the
/// review get them: code, and list markers where the platform reads them; in order.
pub fn protected_ranges(code: &[(usize, usize)], markers: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut ranges = code.to_vec();
    ranges.extend(markers);
    ranges.sort_unstable();
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_managers_are_known_by_each_systems_ids() {
        assert!(is_password_manager(Os::MacOs, "com.1password.1password"));
        assert!(is_password_manager(Os::MacOs, "com.apple.Passwords"));
        assert!(is_password_manager(Os::Linux, "org.keepassxc.KeePassXC"));
        assert!(is_password_manager(Os::Linux, "bitwarden"), "Linux app IDs ignore case");
        assert!(is_password_manager(Os::Linux, "nordpass"));
        assert!(is_password_manager(Os::Windows, "keepassxc.EXE"), "Windows file names ignore case");
        assert!(is_password_manager(Os::Windows, "NordPass.exe"));
        assert!(!is_password_manager(Os::Linux, "1Password.exe"), "another system's ID");
        assert!(!is_password_manager(Os::Windows, "1Password"), "another system's ID");
        assert!(!is_password_manager(Os::Linux, "org.gnome.TextEditor"));
        assert!(!is_password_manager(Os::Windows, "QuillPoint.exe"));
        assert!(!is_password_manager(Os::MacOs, ""));
    }

    #[test]
    fn every_password_manager_is_excluded_by_default() {
        let config = crate::Config::default();
        for id in PASSWORD_MANAGERS.iter().filter_map(|app| app.id(Os::CURRENT)) {
            assert!(config.is_excluded(id), "{id}");
        }
    }

    #[test]
    fn chromium_browsers_match_by_bundle_prefix_on_macos_and_whole_name_on_windows() {
        assert!(is_chromium_browser(Os::MacOs, "com.google.Chrome"));
        assert!(is_chromium_browser(Os::MacOs, "com.google.Chrome.canary"), "a browser's other builds");
        assert!(is_chromium_browser(Os::MacOs, "company.thebrowser.Browser"));
        assert!(!is_chromium_browser(Os::MacOs, "com.apple.Safari"));
        assert!(!is_chromium_browser(Os::MacOs, "com.google.chrome"), "bundle IDs keep their case");
        assert!(is_chromium_browser(Os::Windows, "MSEDGE.EXE"));
        assert!(!is_chromium_browser(Os::Windows, "chrome.exe.bak"), "a whole name");
        assert!(!is_chromium_browser(Os::Windows, "Discord.exe"), "an Electron app is no browser");
        assert!(!is_chromium_browser(Os::Linux, "chromium"), "Linux finds Chromium by its runtime");
    }

    #[test]
    fn a_selection_is_unchanged_only_in_the_same_place_with_the_same_text() {
        let captured = Selection::text("Brynlee sent the OKR draft");
        assert!(selection_unchanged(true, captured, Selection::text("Brynlee sent the OKR draft")));
        assert!(!selection_unchanged(true, captured, Selection::text("Brynlee sent the KPI draft")));
        assert!(!selection_unchanged(false, captured, captured), "moved, or another field");
    }

    #[test]
    fn a_selection_read_with_context_needs_the_same_context() {
        let text: Vec<u16> = "their".encode_utf16().collect();
        let before: Vec<u16> = "Kyndal said ".encode_utf16().collect();
        let after: Vec<u16> = " going".encode_utf16().collect();
        let other: Vec<u16> = "Rhyan said ".encode_utf16().collect();
        let captured = Selection::with_context(&text[..], &before[..], &after[..]);
        assert!(selection_unchanged(true, captured, Selection::with_context(&text[..], &before[..], &after[..])));
        assert!(
            !selection_unchanged(true, captured, Selection::with_context(&text[..], &other[..], &after[..])),
            "the same word somewhere else"
        );
        assert!(!selection_unchanged(true, captured, Selection::text(&text[..])), "context on one side only");
    }

    #[test]
    fn protected_ranges_are_code_and_markers_in_order() {
        assert_eq!(protected_ranges(&[(10, 14), (2, 6)], &[]), [(2, 6), (10, 14)]);
        assert_eq!(protected_ranges(&[(4, 9)], &[(0, 2), (12, 15)]), [(0, 2), (4, 9), (12, 15)]);
        assert!(protected_ranges(&[], &[]).is_empty());
    }
}
