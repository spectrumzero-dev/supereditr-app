//! Finds AI accounts and local models the user already has, so setup is one click.
//!
//! Detection only runs when the user asks for it (onboarding or Settings). Keys found here stay
//! in memory until the user chooses one; the app then stores it in the OS keychain.

use crate::provider::{ProviderKind, Providers};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub kind: ProviderKind,
    /// Where it was found, e.g. "ANTHROPIC_API_KEY in your shell profile".
    pub source: String,
    #[serde(skip)]
    pub api_key: Option<String>,
    /// Masked key for display, e.g. "sk-ant-…9f2c".
    pub key_hint: Option<String>,
    pub base_url: String,
    pub models: Vec<String>,
    pub note: Option<String>,
    /// False when it was found but can't be used yet (for example, a server that isn't running).
    pub ready: bool,
}

impl Candidate {
    fn new(kind: ProviderKind, source: impl Into<String>) -> Self {
        Self {
            kind,
            source: source.into(),
            api_key: None,
            key_hint: None,
            base_url: kind.default_base_url().to_string(),
            models: Vec::new(),
            note: None,
            ready: true,
        }
    }

    fn with_key(mut self, key: &str) -> Self {
        self.key_hint = Some(mask(key));
        self.api_key = Some(key.to_string());
        self
    }
}

pub fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 10 {
        return "••••".into();
    }
    let prefix: String = chars.iter().take_while(|c| **c != '-').take(6).collect();
    let prefix = if chars.get(prefix.len()) == Some(&'-') { format!("{prefix}-") } else { String::new() };
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    format!("{prefix}…{suffix}")
}

const ENV_KEYS: &[(&str, ProviderKind)] = &[
    ("ANTHROPIC_API_KEY", ProviderKind::Anthropic),
    ("OPENAI_API_KEY", ProviderKind::OpenAi),
    ("GEMINI_API_KEY", ProviderKind::Gemini),
    ("GOOGLE_API_KEY", ProviderKind::Gemini),
];

/// API keys exported as environment variables.
pub fn env_candidates(env: &HashMap<String, String>, origin: &str) -> Vec<Candidate> {
    ENV_KEYS
        .iter()
        .filter_map(|(name, kind)| {
            let value = env.get(*name)?.trim();
            (value.len() > 8).then(|| Candidate::new(*kind, format!("{name} in {origin}")).with_key(value))
        })
        .collect()
}

/// An OpenAI API key saved by the Codex CLI when it was set up with a key rather than a
/// ChatGPT sign-in. ChatGPT sign-in tokens are not usable by other apps and are ignored.
pub fn codex_candidate(home: &Path) -> Option<Candidate> {
    let raw = std::fs::read_to_string(home.join(".codex/auth.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let key = value["OPENAI_API_KEY"].as_str()?.trim();
    (key.len() > 8)
        .then(|| Candidate::new(ProviderKind::OpenAi, "Codex CLI settings (~/.codex/auth.json)").with_key(key))
}

#[cfg(not(windows))]
pub fn find_executable(name: &str, path_var: &str, home: &Path) -> Option<PathBuf> {
    let extra = [
        home.join(".local/bin"),
        home.join(".claude/local"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    std::env::split_paths(path_var).chain(extra).map(|dir| dir.join(name)).find(|candidate| candidate.is_file())
}

/// On Windows, a program is `name.exe`: the Claude Code installer's in `~\.local\bin` and Ollama's in
/// `%LOCALAPPDATA%\Programs\Ollama`. An npm `.cmd` shim isn't used: `cmd.exe` would parse the
/// system prompt passed as an argument.
#[cfg(windows)]
pub fn find_executable(name: &str, path_var: &str, home: &Path) -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData/Local"));
    let extra = [home.join(".local/bin"), local.join("Programs/Ollama")];
    let file = format!("{name}.exe");
    std::env::split_paths(path_var).chain(extra).map(|dir| dir.join(&file)).find(|candidate| candidate.is_file())
}

/// Where the Claude desktop app keeps its own copy of Claude Code: one folder per version, so the
/// path changes whenever the app updates.
#[cfg(not(windows))]
pub fn claude_app_code_dir(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Claude/claude-code")
}

/// Where the Claude desktop app keeps its own copy of Claude Code on Windows:
/// `%APPDATA%\Claude\claude-code`, one folder per version. When the app is installed as a package
/// and that folder doesn't exist, Windows keeps the app's roaming data in the package's own
/// `LocalCache\Roaming` folder instead, so that is looked at too.
#[cfg(windows)]
pub fn claude_app_code_dir(home: &Path) -> PathBuf {
    let env = |name: &str| std::env::var_os(name).map(PathBuf::from).filter(|dir| dir.is_absolute());
    let roaming = env("APPDATA").unwrap_or_else(|| home.join(r"AppData\Roaming"));
    let local = env("LOCALAPPDATA").unwrap_or_else(|| home.join(r"AppData\Local"));
    windows_claude_app_code_dir(&roaming, &local)
}

#[cfg(any(windows, test))]
fn windows_claude_app_code_dir(roaming: &Path, local: &Path) -> PathBuf {
    let direct = roaming.join("Claude").join("claude-code");
    if direct.is_dir() {
        return direct;
    }
    let packaged = std::fs::read_dir(local.join("Packages")).into_iter().flatten().flatten().find_map(|entry| {
        let name = entry.file_name();
        let dir =
            ["LocalCache", "Roaming", "Claude", "claude-code"].iter().fold(entry.path(), |dir, part| dir.join(part));
        (name.to_str()?.starts_with("Claude_") && dir.is_dir()).then_some(dir)
    });
    packaged.unwrap_or(direct)
}

/// The Claude Code program's file name in a version folder, with the macOS app's bundle as well.
#[cfg(not(windows))]
const CLAUDE_APP_PROGRAMS: &[&str] = &["claude", "claude.app/Contents/MacOS/claude"];
#[cfg(windows)]
const CLAUDE_APP_PROGRAMS: &[&str] = &["claude.exe"];

/// The newest copy of Claude Code that the Claude desktop app installed for itself, under `dir`
/// (see [`claude_app_code_dir`]). Each version folder holds the program, either directly or one
/// folder further down, and either as a bare `claude` or inside a `claude.app` bundle (`claude.exe`
/// on Windows). Version
/// folders are compared by version number, so 2.10.0 is newer than 2.9.1; a folder whose name isn't
/// a version, or that holds no program (an update still downloading), is skipped.
pub fn claude_app_code(dir: &Path) -> Option<PathBuf> {
    let mut versions: Vec<(Version, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| Some((Version::parse(entry.file_name().to_str()?)?, entry.path())))
        .collect();
    versions.sort_by(|a, b| b.0.cmp(&a.0));
    versions.into_iter().find_map(|(_, folder)| program_in(&folder).or_else(|| program_below(&folder)))
}

/// The program directly inside a folder: `claude`, or `claude.app/Contents/MacOS/claude`
/// (`claude.exe` on Windows).
fn program_in(folder: &Path) -> Option<PathBuf> {
    CLAUDE_APP_PROGRAMS.iter().map(|name| folder.join(name)).find(|p| is_executable(p))
}

/// The program one folder down, where the app keeps a version's build under a folder of its own.
/// With more than one, a build the app marked `.verified` wins, then the most recently written.
fn program_below(folder: &Path) -> Option<PathBuf> {
    std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let sub = entry.path();
            let program = program_in(&sub)?;
            let verified = sub.join(".verified").is_file();
            let written = entry.metadata().and_then(|m| m.modified()).ok();
            Some(((verified, written), program))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, program)| program)
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    path.is_file()
}

/// A version folder's name, `MAJOR.MINOR.PATCH` with an optional `-prerelease` and `+build`. A
/// release sorts after its prereleases.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    numbers: (u64, u64, u64),
    release: bool,
    prerelease: String,
}

impl Version {
    fn parse(name: &str) -> Option<Self> {
        let name = name.split_once('+').map_or(name, |(version, _)| version);
        let (core, prerelease) = name.split_once('-').unwrap_or((name, ""));
        let mut parts = core.split('.').map(|part| {
            (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())).then(|| part.parse::<u64>().ok()).flatten()
        });
        let numbers = (parts.next()??, parts.next()??, parts.next()??);
        if parts.next().is_some() {
            return None;
        }
        Some(Self { numbers, release: prerelease.is_empty(), prerelease: prerelease.to_string() })
    }
}

/// The Claude Code program a profile runs. A copy that came with the Claude desktop app moves to a
/// new version folder when the app updates, so when the saved one is gone, the newest copy there is
/// used instead.
pub fn claude_program(saved: &str) -> String {
    let home = std::env::home_dir().unwrap_or_default();
    resolve_claude_program(saved, &claude_app_code_dir(&home))
}

fn resolve_claude_program(saved: &str, app_dir: &Path) -> String {
    let saved = saved.trim();
    if saved.is_empty() {
        return "claude".into();
    }
    let path = Path::new(saved);
    if path.starts_with(app_dir)
        && !is_executable(path)
        && let Some(newest) = claude_app_code(app_dir)
    {
        return newest.display().to_string();
    }
    saved.to_string()
}

/// Environment of the user's login shell. Apps launched from Finder don't inherit exports from
/// `~/.zshrc`, which is where most people keep API keys.
pub async fn login_shell_env() -> HashMap<String, String> {
    // Windows has no login shell: apps inherit the user's environment variables.
    if cfg!(windows) {
        return HashMap::new();
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let output = tokio::time::timeout(
        Duration::from_secs(4),
        // It runs the user's startup files, so without SuperEditr's permissions: see `crate::process`.
        crate::process::output(&shell, &["-ilc", "env -0"]),
    )
    .await;
    let Ok(Ok(output)) = output else { return HashMap::new() };
    output
        .stdout
        .split(|b| *b == 0)
        .filter_map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            let (key, value) = entry.split_once('=')?;
            Some((key.trim().to_string(), value.to_string()))
        })
        .collect()
}

async fn probe_local(providers: &Providers, kind: ProviderKind, base_url: &str) -> Option<Candidate> {
    let mut profile = crate::provider::ProviderProfile::new(kind);
    profile.base_url = base_url.to_string();
    let models = tokio::time::timeout(Duration::from_millis(800), providers.list_models(&profile)).await.ok()?.ok()?;
    let mut candidate = Candidate::new(kind, format!("running at {base_url}"));
    candidate.base_url = base_url.to_string();
    if models.is_empty() {
        candidate.note = Some("Running, but no models are downloaded yet.".into());
    }
    candidate.models = models;
    Some(candidate)
}

/// Apple's on-device model, when the bundled helper is present. Offered even when it can't be used
/// yet, with the reason, because turning on Apple Intelligence is a one-click fix.
async fn apple_candidate(providers: &Providers) -> Option<Candidate> {
    let profile = crate::provider::ProviderProfile::new(ProviderKind::AppleIntelligence);
    crate::provider::apple_helper_path(&profile.base_url)?;
    let status = providers.apple_status(&profile).await;
    if status.reason.as_deref().is_some_and(|r| r == "osTooOld" || r == "deviceNotEligible" || r == "missing") {
        return None;
    }
    let mut candidate = Candidate::new(ProviderKind::AppleIntelligence, "built into macOS");
    candidate.ready = status.available;
    candidate.note = Some(if status.available {
        "Private and free: runs on this Mac with no download or key.".into()
    } else {
        status.message().unwrap_or_default()
    });
    Some(candidate)
}

/// Runs every detector concurrently. Typically finishes in well under a second, bounded by the
/// login-shell read.
pub async fn scan(providers: &Providers, home: &Path) -> Vec<Candidate> {
    let process_env: HashMap<String, String> = std::env::vars().collect();
    let (shell_env, apple, ollama, lmstudio) = tokio::join!(
        login_shell_env(),
        apple_candidate(providers),
        probe_local(providers, ProviderKind::Ollama, ProviderKind::Ollama.default_base_url()),
        probe_local(providers, ProviderKind::OpenAiCompatible, "http://127.0.0.1:1234/v1"),
    );

    let mut found = env_candidates(&process_env, "the app environment");
    found.extend(env_candidates(&shell_env, "your shell profile"));
    found.extend(codex_candidate(home));

    found.extend(apple);

    let path_var = shell_env.get("PATH").or(process_env.get("PATH")).cloned().unwrap_or_default();
    // The Claude desktop app's own copy, when there's no separate install: it uses the same sign-in.
    let claude = find_executable("claude", &path_var, home)
        .map(|path| (format!("Claude Code at {}", path.display()), path))
        .or_else(|| {
            claude_app_code(&claude_app_code_dir(home)).map(|path| ("Claude Code from the Claude app".into(), path))
        });
    if let Some((source, claude)) = claude {
        let mut candidate = Candidate::new(ProviderKind::ClaudeCli, source);
        candidate.base_url = claude.display().to_string();
        candidate.note = Some("Uses your Claude subscription. Slower to start (about 2–6 s) than an API key.".into());
        found.push(candidate);
    }
    if let Some(mut lm) = lmstudio {
        lm.note.get_or_insert_with(|| "LM Studio or another local OpenAI-compatible server.".into());
        found.push(lm);
    }
    match ollama {
        Some(candidate) => found.push(candidate),
        None => {
            if let Some(path) = find_executable("ollama", &path_var, home) {
                let mut candidate = Candidate::new(ProviderKind::Ollama, format!("installed at {}", path.display()));
                candidate.note = Some("Installed but not running. Start Ollama, then scan again.".into());
                candidate.ready = false;
                found.push(candidate);
            }
        }
    }

    // The same key is often both exported and saved by a CLI; show it once.
    let mut seen = std::collections::HashSet::new();
    found.retain(|c| match &c.api_key {
        Some(key) => seen.insert((c.kind, key.clone())),
        None => true,
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_keys() {
        assert_eq!(mask("sk-ant-api03-abcdefghijklmnop1234"), "sk-…1234");
        assert_eq!(mask("AIzaSyAbcdefghijklmnop9876"), "…9876");
        assert_eq!(mask("short"), "••••");
    }

    #[test]
    fn finds_env_keys() {
        let env = HashMap::from([
            ("ANTHROPIC_API_KEY".to_string(), "sk-ant-0123456789abcdef".to_string()),
            ("OPENAI_API_KEY".to_string(), "".to_string()),
        ]);
        let found = env_candidates(&env, "test");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, ProviderKind::Anthropic);
        assert!(found[0].api_key.is_some());
        let json = serde_json::to_string(&found[0]).unwrap();
        assert!(!json.contains("0123456789abcdef"), "keys never reach the UI");
    }

    /// A temporary folder laid out like the Claude app's copies of Claude Code.
    struct AppDir(PathBuf);

    impl AppDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("supereditr-claude-app-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        /// `relative`, with `/` between its parts, under this folder.
        fn path(&self, relative: &str) -> PathBuf {
            relative.split('/').fold(self.0.clone(), |path, part| path.join(part))
        }

        /// Writes an executable at `relative` and returns its path. `{P}` in `relative` stands for
        /// the program's file name: `claude`, or `claude.exe` on Windows.
        fn program(&self, relative: &str) -> PathBuf {
            let path = self.path(&relative.replace("{P}", CLAUDE_APP_PROGRAMS[0]));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            path
        }

        fn folder(&self, relative: &str) {
            std::fs::create_dir_all(self.path(relative)).unwrap();
        }
    }

    impl Drop for AppDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_claude_apps_newest_version_is_chosen_by_number() {
        let dir = AppDir::new("newest");
        dir.program("2.1.9/{P}");
        let newest = dir.program("2.1.10/{P}");
        dir.program("2.0.99/{P}");
        assert_eq!(claude_app_code(&dir.0), Some(newest));
    }

    #[cfg(not(windows))]
    #[test]
    fn the_claude_apps_build_folder_and_bundle_are_found() {
        let dir = AppDir::new("bundle");
        dir.program("2.1.300/{P}");
        let newest = dir.program("2.1.321/0a1b2c3d4e5f/claude.app/Contents/MacOS/{P}");
        std::fs::write(dir.0.join("2.1.321/0a1b2c3d4e5f/.verified"), "").unwrap();
        dir.program("2.1.321/9f8e7d6c5b4a/claude.app/Contents/MacOS/{P}");
        assert_eq!(claude_app_code(&dir.0), Some(newest), "the build marked verified wins");
    }

    #[test]
    fn an_empty_version_folder_is_skipped() {
        let dir = AppDir::new("empty");
        let installed = dir.program("2.1.320/{P}");
        dir.folder("2.1.321");
        dir.folder("2.1.322/0123abcd");
        assert_eq!(claude_app_code(&dir.0), Some(installed));
    }

    #[test]
    fn a_folder_that_isnt_a_version_is_skipped() {
        let dir = AppDir::new("names");
        let installed = dir.program("2.1.320/{P}");
        dir.program("latest/{P}");
        dir.program("9.0/{P}");
        dir.program("10.0.0.1/{P}");
        dir.program(".tmp-3.0.0/{P}");
        assert_eq!(claude_app_code(&dir.0), Some(installed));
        assert_eq!(claude_app_code(&dir.0.join("missing")), None);
    }

    #[test]
    fn a_release_is_newer_than_its_prerelease() {
        let dir = AppDir::new("prerelease");
        dir.program("3.0.0-beta.2/{P}");
        let release = dir.program("3.0.0/{P}");
        assert_eq!(claude_app_code(&dir.0), Some(release));
        assert!(Version::parse("3.0.0+build.7").is_some());
    }

    #[test]
    fn the_claude_apps_build_folders_are_found() {
        // As on Windows, `%APPDATA%\Claude\claude-code\<version>\<build>\claude.exe`, with whichever
        // program name this system uses.
        let dir = AppDir::new("build");
        dir.program("2.1.300/0f0f0f0f/{P}");
        let verified = dir.program("2.1.321/0a1b2c3d4e5f/{P}");
        std::fs::write(dir.path("2.1.321/0a1b2c3d4e5f/.verified"), "").unwrap();
        dir.program("2.1.321/9f8e7d6c5b4a/{P}");
        assert_eq!(claude_app_code(&dir.0), Some(verified), "the build marked verified wins");
        // Without a mark, the most recently written build wins.
        let dir = AppDir::new("unmarked");
        dir.program("2.1.321/0a1b2c3d4e5f/{P}");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let newer = dir.program("2.1.321/9f8e7d6c5b4a/{P}");
        assert_eq!(claude_app_code(&dir.0), Some(newer));
    }

    #[test]
    fn a_packaged_claude_apps_folder_is_found_on_windows() {
        let dir = AppDir::new("packaged");
        let (roaming, local) = (dir.path("Roaming"), dir.path("Local"));
        let direct = dir.path("Roaming/Claude/claude-code");
        assert_eq!(windows_claude_app_code_dir(&roaming, &local), direct, "neither exists");
        dir.folder("Local/Packages/ClaudeHelper_1a2b/LocalCache/Roaming/Claude/claude-code");
        dir.folder("Local/Packages/Claude_pzs8sxrjxfjjc/LocalCache/Roaming/Claude/claude-code");
        let packaged = dir.path("Local/Packages/Claude_pzs8sxrjxfjjc/LocalCache/Roaming/Claude/claude-code");
        assert_eq!(windows_claude_app_code_dir(&roaming, &local), packaged);
        dir.folder("Roaming/Claude/claude-code");
        assert_eq!(windows_claude_app_code_dir(&roaming, &local), direct, "the ordinary folder comes first");
    }

    #[test]
    fn a_claude_app_copy_that_moved_is_found_again() {
        let dir = AppDir::new("moved");
        let old = dir.path("2.1.320/claude").display().to_string();
        let newest = dir.program("2.1.321/{P}");
        assert_eq!(resolve_claude_program(&old, &dir.0), newest.display().to_string());
        // A copy that is still there stays as saved, and so does any other program.
        let kept = dir.program("2.1.300/{P}").display().to_string();
        assert_eq!(resolve_claude_program(&kept, &dir.0), kept);
        assert_eq!(resolve_claude_program("/opt/tools/claude", &dir.0), "/opt/tools/claude");
        assert_eq!(resolve_claude_program("", &dir.0), "claude");
    }

    #[test]
    fn reads_codex_api_key_only() {
        let dir = std::env::temp_dir().join(format!("supereditr-codex-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".codex")).unwrap();
        std::fs::write(dir.join(".codex/auth.json"), r#"{"OPENAI_API_KEY": null, "tokens": {"access_token": "x"}}"#)
            .unwrap();
        assert!(codex_candidate(&dir).is_none());
        std::fs::write(dir.join(".codex/auth.json"), r#"{"OPENAI_API_KEY": "sk-proj-0123456789"}"#).unwrap();
        assert_eq!(codex_candidate(&dir).unwrap().kind, ProviderKind::OpenAi);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
