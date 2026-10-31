// Mirrors the serialized types in crates/core and src-tauri.

export type ChangeKind = "spelling" | "grammar" | "punctuation" | "capitalization" | "spacing" | "style";

export type Segment =
  | { type: "equal"; text: string }
  | {
      type: "change";
      id: number;
      old: string;
      new: string;
      kind: ChangeKind;
      oldStart: number;
      note?: string;
      /** Made by the offline checker's final pass over an AI's revision, not by the AI. */
      ruleBased?: boolean;
    };

export interface DiffView {
  segments: Segment[];
  pending: string;
  changeCount: number;
}

export type EngineEvent =
  | { type: "local"; view: DiffView; ms: number }
  | { type: "partial"; view: DiffView; firstTokenMs: number; source: string }
  | {
      type: "done";
      view: DiffView;
      text: string;
      source: string;
      firstTokenMs: number | null;
      totalMs: number;
      changedTokens: string[];
      gradeBefore: number | null;
      gradeAfter: number | null;
      /** Why Apple Intelligence answered instead of the chosen AI, when it did. */
      backupFor: string | null;
      /** Changes that would drop the author's own terms; they start out rejected. */
      kept: number[];
      keptTerms: string[];
      /** Words the dictionary doesn't know and the word list doesn't have; see `offerToKeep`. */
      learnable: string[];
    }
  | { type: "failed"; message: string; localOnly: boolean };

export interface PresetInfo {
  id: string;
  name: string;
}

export interface SessionPayload {
  id: number;
  text: string;
  appName: string;
  canApply: boolean;
  auto: boolean;
  presetId: string;
  presets: PresetInfo[];
  showTiming: boolean;
  provider: string | null;
}

export interface StatusPayload {
  id: number;
  kind: "notice" | "applied" | "error";
  message: string;
}

export type ProviderKind = "anthropic" | "open_ai" | "gemini" | "ollama" | "open_ai_compatible" | "claude_cli" | "apple_intelligence";

export interface ProviderProfile {
  id: string;
  kind: ProviderKind;
  label: string;
  base_url: string;
  model: string;
  extra_body?: unknown;
}

export interface Preset {
  id: string;
  name: string;
  instruction: string;
  builtin: boolean;
  shortcut?: string | null;
}

export interface Config {
  providers: ProviderProfile[];
  active_provider: string | null;
  presets: Preset[];
  default_preset: string;
  style_notes: string;
  dialect: string;
  hotkeys: { review: string; quick_fix: string };
  auto_popup: boolean;
  auto_min_chars: number;
  /** With nothing selected, the shortcuts select and review the sentence before the cursor. */
  select_sentence: boolean;
  /** Apply a proofread by changing only the words it fixes, keeping the formatting around them. */
  keep_formatting: boolean;
  /** Names and terms to keep exactly as written: a word or a phrase. */
  words: string[];
  excluded_apps: string[];
  /** Default exclusions already offered; see `offered_exclusions` in crates/core/src/config.rs. */
  offered_exclusions: string[];
  /** Built-in styles already offered, so a deleted one stays deleted. */
  offered_presets: string[];
  show_timing: boolean;
  /** Keep capture.log for diagnosing a problem in a particular app; see crates/core/src/config.rs. */
  diagnostic_log: boolean;
  /** Opening at login was turned on once as the default; see crates/core/src/config.rs. */
  offered_launch_at_login: boolean;
  onboarded: boolean;
}

/** An installed app, as Settings → Privacy lists it (see `apps.rs`). */
export interface AppInfo {
  bundleId: string;
  name: string;
  /** A PNG data URL, or null if the icon couldn't be drawn. */
  icon: string | null;
}

/** The apps chosen in Settings → Privacy's open panel: none if the user cancelled. */
export interface PickedApps {
  apps: AppInfo[];
  /** Names of chosen apps without a bundle ID, which SuperEditr can't recognize. */
  skipped: string[];
}

export interface SettingsPayload {
  config: Config;
  keys: Record<string, boolean>;
  accessibility: boolean;
  version: string;
  platform: string;
  /** The revision `config` was saved at; see `save_settings` in commands.rs. */
  revision: number;
}

/** What became of a save: the new revision, or null when the app refused a save based on an older one. */
export interface Saved {
  revision: number | null;
  problems: string[];
}

/** Whether an AI is ready: key saved and server answering (see `check` in commands.rs). */
export interface ProviderCheck {
  ready: boolean;
  problem: string | null;
  models: string[];
}

/** Apple Intelligence, which reviews unless another AI is chosen. */
export interface BuiltinAi {
  /** False on Macs that can never run it. */
  supported: boolean;
  available: boolean;
  message: string | null;
}

export interface Candidate {
  index: number;
  kind: ProviderKind;
  label: string;
  source: string;
  keyHint: string | null;
  baseUrl: string;
  models: string[];
  note: string | null;
  ready: boolean;
  /** The provider already added from this account, if any. */
  profileId: string | null;
}
