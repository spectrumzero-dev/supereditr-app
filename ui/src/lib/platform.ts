/** Whether the app runs on macOS. Until Settings reads the platform the app reports
 * (`get_settings`), the web view's user agent decides: a Linux web view says so. */
let mac = typeof navigator === "undefined" || !/Linux|X11/.test(navigator.userAgent);

export const isMac = () => mac;

/** Sets the platform from the app's report ("macos", "linux", …). */
export function setPlatform(platform: string) {
  mac = platform === "macos";
}

/** Words that differ between macOS and Linux. */
export interface Words {
  /** "on this Mac" */
  device: string;
  /** Where API keys are kept. */
  keyStore: string;
  /** Where SuperEditr's icon sits. */
  tray: string;
  /** The operating system, as a name for shortcuts it uses itself. */
  system: string;
  /** The panel's apply and copy keys. */
  applyKey: string;
  copyKey: string;
}

const macWords: Words = {
  device: "this Mac",
  keyStore: "your Keychain",
  tray: "the menu bar",
  system: "macOS",
  applyKey: "⌥↩",
  copyKey: "⌘C",
};

const linuxWords: Words = {
  device: "this computer",
  keyStore: "your keyring",
  tray: "the system tray",
  system: "your desktop",
  applyKey: "Alt+Enter",
  copyKey: "Ctrl+C",
};

export const words = (): Words => (mac ? macWords : linuxWords);
