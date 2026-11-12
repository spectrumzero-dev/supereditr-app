export type Platform = "macos" | "linux" | "windows";

/** Which platform the app runs on. Until Settings reads the platform the app reports
 * (`get_settings`), the web view's user agent decides: a Windows or Linux web view says so. */
function fromUserAgent(): Platform {
  if (typeof navigator === "undefined") return "macos";
  if (/Windows/.test(navigator.userAgent)) return "windows";
  if (/Linux|X11/.test(navigator.userAgent)) return "linux";
  return "macos";
}

let current: Platform = fromUserAgent();

export const isMac = () => current === "macos";
export const isLinux = () => current === "linux";
export const isWindows = () => current === "windows";

/** Sets the platform from the app's report ("macos", "linux", "windows"). Another Unix reads as
 * Linux. */
export function setPlatform(platform: string) {
  current = platform === "macos" || platform === "windows" ? platform : "linux";
}

let keyLabels: Record<string, string> = {};

/** What keys saved by virtual key show on the current layout (Windows): "VkBA" is "Ü" on German. */
export function setKeyLabels(labels: Record<string, string>) {
  keyLabels = labels;
}

/** The label for a key part of an accelerator saved by virtual key, if the layout has one. A key
 * the layout doesn't label shows its virtual key. */
export function keyLabel(part: string): string | undefined {
  return keyLabels[part] ?? (/^Vk[0-9A-F]{2}$/.test(part) ? `Key ${part.slice(2)}` : undefined);
}

/** Words that differ between macOS, Linux and Windows. */
export interface Words {
  /** "on this Mac" */
  device: string;
  /** "after you restart your Mac" */
  yourDevice: string;
  /** Where API keys are kept. */
  keyStore: string;
  /** Where SuperEditr's icon sits. */
  tray: string;
  /** The operating system, as a name for shortcuts it uses itself. */
  system: string;
  /** The panel's apply and copy keys. `applyKey` works in every panel. */
  applyKey: string;
  copyKey: string;
  /** The keys that apply in a panel that has the keyboard, the main one first. */
  applyKeys: string[];
  /** The panel's Tab and Esc, as the platform names them. */
  tabKey: string;
  escKey: string;
  /** Claude Code isn't signed in (`claude_sign_in`). */
  claudeSignIn: string;
}

const macWords: Words = {
  device: "this Mac",
  yourDevice: "your Mac",
  keyStore: "your Keychain",
  tray: "the menu bar",
  system: "macOS",
  applyKey: "⌥↩",
  copyKey: "⌘C",
  applyKeys: ["↩", "⌥↩"],
  tabKey: "tab",
  escKey: "esc",
  claudeSignIn: "Sign in to Claude Code: run `claude` in Terminal and use /login",
};

const linuxWords: Words = {
  device: "this computer",
  yourDevice: "your computer",
  keyStore: "your keyring",
  tray: "the system tray",
  system: "your desktop",
  applyKey: "Alt+Enter",
  copyKey: "Ctrl+C",
  applyKeys: ["Enter", "Alt+Enter"],
  tabKey: "tab",
  escKey: "esc",
  claudeSignIn: "Sign in to Claude Code: run `claude` in Terminal and use /login",
};

const windowsWords: Words = {
  device: "this PC",
  yourDevice: "your PC",
  keyStore: "Windows Credential Manager",
  tray: "the notification area",
  system: "Windows",
  applyKey: "Alt+Enter",
  copyKey: "Ctrl+C",
  // Enter works too while the panel has the keyboard; Alt+Enter works always, so it's the one shown.
  applyKeys: ["Alt+Enter"],
  tabKey: "Tab",
  escKey: "Esc",
  claudeSignIn: "Claude Code isn’t signed in to your Claude account yet",
};

export const words = (): Words => (current === "macos" ? macWords : current === "windows" ? windowsWords : linuxWords);
