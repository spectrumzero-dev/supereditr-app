import { keySymbols } from "./diff";

/** Converts a keydown into an accelerator string ("Control+Alt+E"), or null if incomplete. */
export function acceleratorFromEvent(e: KeyboardEvent): string | null {
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Control");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Command");
  let key: string | null = null;
  if (/^Key[A-Z]$/.test(e.code)) key = e.code.slice(3);
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
 * ("Command+E Command+E") repeats the key: ⌘ E E. */
export function keycaps(accelerator: string): string[] {
  const [first, second] = accelerator.trim().split(/\s+/);
  if (!first) return [];
  const parts = keySymbols(first).match(/[⌃⌥⇧⌘]|[^⌃⌥⇧⌘]+/g) ?? [];
  return second ? [...parts, parts.at(-1) ?? ""] : parts;
}
