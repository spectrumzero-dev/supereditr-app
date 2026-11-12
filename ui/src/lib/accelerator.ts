import { keySymbols } from "./diff";
import { isMac, isWindows } from "./platform";

/** Windows virtual keys with names of their own; other keys are saved as `VkXX`. */
const WINDOWS_NAMED: Record<number, string> = {
  0x20: "Space",
  0x0d: "Enter",
  0x09: "Tab",
  0x2d: "Insert",
  0x24: "Home",
  0x23: "End",
  0x21: "PageUp",
  0x22: "PageDown",
  0x25: "ArrowLeft",
  0x26: "ArrowUp",
  0x27: "ArrowRight",
  0x28: "ArrowDown",
};

/** The accelerator name for the virtual key Windows reported (`keyCode` in a Windows web view):
 * letters, digits and function keys by name, other keys that type a character as `VkXX`, so the
 * shortcut is registered on the key the user pressed whatever the layout (see keys.rs). */
export function windowsKeyName(vk: number): string | null {
  if ((vk >= 0x30 && vk <= 0x39) || (vk >= 0x41 && vk <= 0x5a)) return String.fromCharCode(vk);
  if (vk >= 0x70 && vk <= 0x87) return `F${vk - 0x6f}`;
  if (WINDOWS_NAMED[vk]) return WINDOWS_NAMED[vk];
  if ((vk >= 0xba && vk <= 0xc0) || (vk >= 0xdb && vk <= 0xdf) || vk === 0xe1 || vk === 0xe2) {
    return `Vk${vk.toString(16).toUpperCase().padStart(2, "0")}`;
  }
  return null;
}

/** Converts a keydown into an accelerator string ("Control+Alt+E"), or null if incomplete. */
export function acceleratorFromEvent(e: KeyboardEvent): string | null {
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Control");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Command");
  let key: string | null = null;
  if (isWindows()) key = windowsKeyName(e.keyCode);
  else if (/^Key[A-Z]$/.test(e.code)) key = e.code.slice(3);
  else if (/^Digit[0-9]$/.test(e.code)) key = e.code.slice(5);
  else if (/^F([1-9]|1[0-9]|20)$/.test(e.code)) key = e.code;
  else if (["Space", "Enter", "Backslash", "Slash", "Period", "Comma", "Semicolon", "Quote", "BracketLeft", "BracketRight", "Minus", "Equal", "Backquote"].includes(e.code)) key = e.code;
  if (!key) return null;
  const isFunctionKey = key.startsWith("F") && key.length > 1;
  // Plain letters would hijack normal typing everywhere; require a modifier other than Shift.
  if (!isFunctionKey && !mods.some((m) => m !== "Shift")) return null;
  return [...mods, key].join("+");
}

/** Keycaps for an accelerator: each modifier and the key on its own cap; a double press
 * ("Command+E Command+E") repeats the key: ⌘ E E. Off macOS, the caps are names: Super Alt E. */
export function keycaps(accelerator: string, mac = isMac()): string[] {
  const [first, second] = accelerator.trim().split(/\s+/);
  if (!first) return [];
  const parts = mac
    ? (keySymbols(first, mac).match(/[⌃⌥⇧⌘]|[^⌃⌥⇧⌘]+/g) ?? [])
    : keySymbols(first, mac).split("+").filter(Boolean);
  return second ? [...parts, parts.at(-1) ?? ""] : parts;
}
