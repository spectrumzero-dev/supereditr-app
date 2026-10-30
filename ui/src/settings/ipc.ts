import type { BuiltinAi, Candidate, ProviderCheck, Config, EngineEvent, Preset, ProviderKind, ProviderProfile, SettingsPayload } from "../lib/types";

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
  onboarded: false,
};

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (inTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(cmd, args);
  }
  // Browser preview: just enough behavior to design the screens.
  const demo: Record<string, () => unknown> = {
    get_settings: () => ({ config: demoConfig, keys: { anthropic: true }, accessibility: false, version: "0.1.0", platform: "macos" }),
    save_settings: () => {
      demoConfig = args?.config as Config;
      return [];
    },
    scan_accounts: () => [
      { index: 0, kind: "anthropic", label: "Claude (Anthropic API)", source: "ANTHROPIC_API_KEY in your shell profile", keyHint: "sk-…9f2c", baseUrl: "", models: [], note: null, ready: true, profileId: null },
      { index: 1, kind: "claude_cli", label: "Claude Code subscription", source: "Claude Code at ~/.local/bin/claude", keyHint: null, baseUrl: "", models: [], note: "Uses your Claude subscription. Slower to start (about 2–6 s) than an API key.", ready: true, profileId: null },
      { index: 2, kind: "ollama", label: "Ollama (local)", source: "installed at /opt/homebrew/bin/ollama", keyHint: null, baseUrl: "", models: [], note: "Installed but not running. Start Ollama, then scan again.", ready: false, profileId: null },
    ],
    test_provider: () => ({ firstTokenMs: 284, totalMs: 412, output: "This is a quick test." }),
    list_models: () => ["claude-haiku-5-5", "claude-sonnet-5-5", "claude-opus-5-5"],
    accessibility_status: () => false,
    check_shortcut: () => (args?.accelerator === "Command+Space" ? "macOS uses this for Spotlight." : null),
    default_presets: () => demoConfig.presets.map((p) => ({ ...p })),
    builtin_ai: () => ({ supported: true, available: true, message: null }),
    check_provider: () => ({ ready: false, problem: "Add an API key", models: [] }),
  };
  return (demo[cmd]?.() ?? null) as T;
}

export const api = {
  get: () => invoke<SettingsPayload>("get_settings"),
  save: (config: Config) => invoke<string[]>("save_settings", { config }),
  setKey: (profileId: string, key: string | null) => invoke<void>("set_api_key", { profileId, key }),
  /** Why a shortcut can't be used for `slot` ("review", "quick_fix", "preset:<id>"), or null. */
  checkShortcut: (accelerator: string, slot: string) => invoke<string | null>("check_shortcut", { accelerator, slot }),
  defaultPresets: () => invoke<Preset[]>("default_presets"),
  scan: () => invoke<Candidate[]>("scan_accounts"),
  adopt: (index: number) => invoke<ProviderProfile>("adopt_candidate", { index }),
  addProvider: (kind: ProviderKind) => invoke<ProviderProfile>("add_provider", { kind }),
  removeProvider: (profileId: string) => invoke<void>("remove_provider", { profileId }),
  checkProvider: (profileId: string) => invoke<ProviderCheck>("check_provider", { profileId }),
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
  openAccessibility: () => invoke<void>("open_accessibility_settings"),
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
