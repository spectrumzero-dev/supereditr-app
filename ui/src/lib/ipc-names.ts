// The commands the app registers and the events it emits, by name, and what each event carries.
// tests/contracts/ipc-names.json lists the same names; contracts.test.ts checks this file against it
// and src-tauri/src/ipc_contract.rs checks the app. The windows' `invoke` and `listen` wrappers take
// only these names, so a misspelt one doesn't compile.
import type { EnginePayload, GuideSide, OllamaPullProgress, PanelKeyPayload, SessionPayload, StatusPayload, UpdateStatus } from "./types";

export const commands = [
  "accessibility_status",
  "activate_provider",
  "add_provider",
  "add_word",
  "adopt_candidate",
  "app_info",
  "apply_text",
  "builtin_ai",
  "check_protected_tokens",
  "check_provider",
  "check_shortcut",
  "claude_sign_in",
  "close_guide",
  "close_update_window",
  "copy_text",
  "default_presets",
  "dismiss",
  "drag_app",
  "fit_settings_height",
  "focus_overlay",
  "get_settings",
  "guide_accessibility",
  "hide_tips",
  "launch_at_login",
  "linux_setup",
  "open_apple_intelligence_settings",
  "open_settings",
  "overlay_typing",
  "password_managers",
  "pause_shortcuts",
  "pick_apps",
  "pull_ollama_model",
  "remove_omarchy_setup",
  "remove_provider",
  "resize_overlay",
  "review_playground",
  "run_preset",
  "save_settings",
  "scan_accounts",
  "set_api_key",
  "set_launch_at_login",
  "set_up_omarchy",
  "test_provider",
  "update_now",
  "update_status",
  "use_builtin_provider",
] as const;

/** A command the app registers (`generate_handler!` in src-tauri/src/lib.rs). */
export type Command = (typeof commands)[number];

/** What each event carries; `null` for an event that carries nothing (`()` in Rust). */
export interface EventPayloads {
  "guide:side": GuideSide;
  /** A panel command from a shortcut, such as "apply". */
  "overlay:command": string;
  "overlay:engine": EnginePayload;
  /** The review the panel should take keyboard focus for. */
  "overlay:focus": number;
  "overlay:key": PanelKeyPayload;
  /** The review that couldn't be applied with the clipboard put aside. */
  "overlay:replace-anyway": number;
  "overlay:session": SessionPayload;
  "overlay:status": StatusPayload;
  "settings:accessibility-granted": null;
  /** The revised text for the "Try it now" box. */
  "settings:playground-apply": string;
  "settings:pull": OllamaPullProgress;
  "settings:reload": null;
  "settings:review-shortcut": null;
  /** The Settings section to show. */
  "settings:section": string;
  "update:status": UpdateStatus;
}

/** An event the app emits, named `<window>:<event>`. */
export type EventName = keyof EventPayloads;

export const events = [
  "guide:side",
  "overlay:command",
  "overlay:engine",
  "overlay:focus",
  "overlay:key",
  "overlay:replace-anyway",
  "overlay:session",
  "overlay:status",
  "settings:accessibility-granted",
  "settings:playground-apply",
  "settings:pull",
  "settings:reload",
  "settings:review-shortcut",
  "settings:section",
  "update:status",
] as const satisfies readonly EventName[];

// Every event that has a payload type above is in `events` too.
const everyEventListed: Exclude<EventName, (typeof events)[number]> extends never ? true : false = true;
void everyEventListed;
