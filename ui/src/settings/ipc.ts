import type { AppInfo, BuiltinAi, Candidate, ProviderCheck, Config, EngineEvent, LinuxSetup, Preset, ProviderKind, PickedApps, ProviderProfile, Saved, SettingsPayload } from "../lib/types";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

let demoConfig: Config = {
  providers: [{ id: "anthropic", kind: "anthropic", label: "Claude (Anthropic API)", base_url: "https://api.anthropic.com", model: "claude-haiku-5-5" }],
  active_provider: null,
  presets: [
    { id: "proofread", name: "Proofread", instruction: "Fix spelling, grammar, punctuation, capitalization, and clear word-choice mistakes. Make the smallest edits that make the text correct.", builtin: true },
    { id: "polish", name: "Polish", instruction: "Fix every error, then improve clarity and flow.", builtin: true },
    { id: "shorter", name: "Shorter", instruction: "Make the text 30–50% shorter.", builtin: true },
    { id: "friendly", name: "Friendlier", instruction: "Make the tone warm and friendly.", builtin: true },
    { id: "professional", name: "Professional", instruction: "Make the tone clear and professional.", builtin: true },
    { id: "simplify", name: "Simplify", instruction: "Rewrite in plain language.", builtin: true },
    { id: "english", name: "To English", instruction: "Translate into natural English.", builtin: true },
  ],
  default_preset: "proofread",
  style_notes: "",
  dialect: "American English",
  hotkeys: { review: "Command+E Command+E", quick_fix: "Control+Alt+Shift+E" },
  auto_popup: true,
  auto_min_chars: 12,
  select_sentence: true,
  keep_formatting: true,
  words: ["Rhyan", "1234 Glenmoor Ct"],
  excluded_apps: ["com.1password.1password", "com.apple.Terminal"],
  offered_exclusions: ["com.1password.1password", "com.apple.Terminal"],
  offered_presets: ["proofread", "polish", "shorter", "friendly", "professional", "simplify", "english"],
  show_timing: true,
  diagnostic_log: false,
  offered_launch_at_login: true,
  onboarded: false,
};

let demoLaunchAtLogin = true;
let demoRevision = 0;

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (inTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(cmd, args);
  }
  // Browser preview: just enough behavior to design the screens.
  const demo: Record<string, () => unknown> = {
    get_settings: () => ({ config: demoConfig, keys: { anthropic: true }, accessibility: false, version: "0.1.0", platform: "macos", logPath: "/Users/rhyan/Library/Logs/SuperEditr/capture.log", revision: demoRevision }),
    save_settings: () => {
      if (args?.revision != null && args.revision !== demoRevision) return { revision: null, problems: [] };
      demoConfig = args?.config as Config;
      return { revision: ++demoRevision, problems: [] };
    },
    scan_accounts: () => [
      { index: 0, kind: "anthropic", label: "Claude (Anthropic API)", source: "ANTHROPIC_API_KEY in your shell profile", keyHint: "sk-…9f2c", baseUrl: "", models: [], note: null, ready: true, profileId: null },
      { index: 1, kind: "claude_cli", label: "Claude Code subscription", source: "Claude Code at ~/.local/bin/claude", keyHint: null, baseUrl: "", models: [], note: "Uses your Claude subscription. Slower to start (about 2–6 s) than an API key.", ready: true, profileId: null },
      { index: 2, kind: "ollama", label: "Ollama (local)", source: "installed at /opt/homebrew/bin/ollama", keyHint: null, baseUrl: "", models: [], note: "Installed but not running. Start Ollama, then scan again.", ready: false, profileId: null },
    ],
    test_provider: () => ({ firstTokenMs: 284, totalMs: 412, output: "This is a quick test." }),
    list_models: () => ["claude-haiku-5-5", "claude-sonnet-5-5", "claude-opus-5-5"],
    accessibility_status: () => false,
    launch_at_login: () => demoLaunchAtLogin,
    set_launch_at_login: () => (demoLaunchAtLogin = Boolean(args?.enabled)),
    check_shortcut: () => (args?.accelerator === "Command+Space" ? "macOS uses this for Spotlight." : null),
    default_presets: () => demoConfig.presets.map((p) => ({ ...p })),
    builtin_ai: () => ({ supported: true, available: true, message: null }),
    check_provider: () => ({ ready: false, problem: "Add an API key", code: null, models: [] }),
    claude_sign_in: () => undefined,
    app_info: () => {
      const names: Record<string, string> = { "com.1password.1password": "1Password", "com.apple.Terminal": "Terminal", "com.apple.Notes": "Notes" };
      const ids = (args?.bundleIds as string[]).filter((id) => id in names);
      return Object.fromEntries(ids.map((id) => [id, { bundleId: id, name: names[id], icon: null }]));
    },
    pick_apps: () => ({ apps: [{ bundleId: "com.apple.Notes", name: "Notes", icon: null }], skipped: [] }),
    linux_setup: () => ({
      desktop: "Omarchy 4.0.6 on Hyprland 0.56.2",
      omarchy: true,
      checks: [
        { name: "Hyprland", ok: true, detail: "0.56.2" },
        { name: "Accessibility", ok: true, detail: "On for this session, so apps opened from now on expose their text" },
        { name: "Web pages and Electron apps", ok: false, detail: "--force-renderer-accessibility isn’t in ~/.config/chromium-flags.conf. Set up Omarchy adds it." },
      ],
      changes: [
        { path: "~/.config/chromium-flags.conf", what: "Add --force-renderer-accessibility, with a comment marking it as SuperEditr’s.", needed: true },
        { path: "~/.config/autostart/supereditr.desktop", what: "Open SuperEditr at login.", needed: true },
      ],
      restart: [{ name: "Chromium", reason: "started without --force-renderer-accessibility" }],
    }),
    set_up_omarchy: () => ["Added --force-renderer-accessibility to ~/.config/chromium-flags.conf"],
    remove_omarchy_setup: () => [],
  };
  return (demo[cmd]?.() ?? null) as T;
}

export const api = {
  get: () => invoke<SettingsPayload>("get_settings"),
  /** Saves the whole config; with `revision`, only if the app hasn't saved it since. */
  save: (config: Config, revision?: number) => invoke<Saved>("save_settings", { config, revision: revision ?? null }),
  setKey: (profileId: string, key: string | null) => invoke<void>("set_api_key", { profileId, key }),
  /** Why a shortcut can't be used for `slot` ("review", "quick_fix", "preset:<id>"), or null. */
  checkShortcut: (accelerator: string, slot: string) => invoke<string | null>("check_shortcut", { accelerator, slot }),
  defaultPresets: () => invoke<Preset[]>("default_presets"),
  /** Closes the review panel, if one is open. */
  dismissReview: () => invoke<void>("dismiss", { id: null }),
  scan: () => invoke<Candidate[]>("scan_accounts"),
  adopt: (index: number) => invoke<ProviderProfile>("adopt_candidate", { index }),
  addProvider: (kind: ProviderKind) => invoke<ProviderProfile>("add_provider", { kind }),
  removeProvider: (profileId: string) => invoke<void>("remove_provider", { profileId }),
  checkProvider: (profileId: string) => invoke<ProviderCheck>("check_provider", { profileId }),
  /** Starts Claude Code's own sign-in in a console window (Windows). */
  claudeSignIn: (profileId: string) => invoke<void>("claude_sign_in", { profileId }),
  activateProvider: (profileId: string) => invoke<ProviderCheck>("activate_provider", { profileId }),
  models: (profileId: string) => invoke<string[]>("list_models", { profileId }),
  test: (profileId: string) => invoke<{ firstTokenMs: number | null; totalMs: number; output: string }>("test_provider", { profileId }),
  pull: (baseUrl: string, model: string) => invoke<void>("pull_ollama_model", { baseUrl, model }),
  builtinAi: () => invoke<BuiltinAi>("builtin_ai"),
  /** `rect` is the box's frame within the window, in CSS pixels. */
  reviewPlayground: (text: string, rect: { x: number; y: number; width: number; height: number }) =>
    invoke<void>("review_playground", { text, rect }),
  fitHeight: (height: number) => invoke<void>("fit_settings_height", { height }),
  pauseShortcuts: (paused: boolean) => invoke<void>("pause_shortcuts", { paused }),
  openAppleIntelligence: () => invoke<void>("open_apple_intelligence_settings"),
  accessibility: (prompt: boolean) => invoke<boolean>("accessibility_status", { prompt }),
  launchAtLogin: () => invoke<boolean>("launch_at_login"),
  /** Returns whether opening at login is on afterwards. */
  setLaunchAtLogin: (enabled: boolean) => invoke<boolean>("set_launch_at_login", { enabled }),
  openAccessibility: () => invoke<void>("open_accessibility_settings"),
  /** The Linux setup page's checks; null on other systems. */
  linuxSetup: () => invoke<LinuxSetup | null>("linux_setup"),
  /** Returns what it changed. */
  setUpOmarchy: () => invoke<string[]>("set_up_omarchy"),
  removeOmarchySetup: () => invoke<string[]>("remove_omarchy_setup"),
  /** Names and icons of the installed apps among `bundleIds`, by bundle ID. */
  appInfo: (bundleIds: string[]) => invoke<Record<string, AppInfo>>("app_info", { bundleIds }),
  /** Opens a panel on /Applications and returns the apps chosen there. */
  pickApps: () => invoke<PickedApps>("pick_apps"),
  /** Opens System Settings → Accessibility with the drag-and-drop guide beside it. */
  guideAccessibility: () => invoke<void>("guide_accessibility"),
  preview: (id: number, text: string, presetId: string) => invoke<void>("preview_review", { id, text, presetId }),
  openUrl: async (url: string) => {
    if (!inTauri) return void window.open(url, "_blank");
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("plugin:opener|open_url", { url });
  },
  async on<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
    if (!inTauri) return () => {};
    const { listen } = await import("@tauri-apps/api/event");
    return listen<T>(event, (e) => handler(e.payload));
  },
};

export type PreviewEvent = { id: number; event: EngineEvent };
