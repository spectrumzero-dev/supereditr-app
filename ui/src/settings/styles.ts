import type { Preset } from "../lib/types";

/** What resetting the styles to the defaults would throw away. Deleted built-ins aren't a loss:
 * the reset brings them back. */
export interface ResetLosses {
  /** Built-in styles whose name, instruction, or shortcut was changed. */
  edited: string[];
  /** Styles the user added. */
  custom: string[];
  /** The built-ins were put in a different order. */
  reordered: boolean;
}

export function resetLosses(current: Preset[], defaults: Preset[]): ResetLosses {
  const byId = new Map(defaults.map((d) => [d.id, d]));
  const edited: string[] = [];
  const custom: string[] = [];
  for (const p of current) {
    const d = byId.get(p.id);
    if (!d) custom.push(p.name);
    else if (p.name !== d.name || p.instruction !== d.instruction || (p.shortcut ?? null) !== (d.shortcut ?? null)) edited.push(p.name);
  }
  const order = current.filter((p) => byId.has(p.id)).map((p) => p.id);
  const defaultOrder = defaults.map((d) => d.id).filter((id) => order.includes(id));
  return { edited, custom, reordered: order.join() !== defaultOrder.join() };
}

export const losesWork = (l: ResetLosses) => l.edited.length > 0 || l.custom.length > 0 || l.reordered;

/** The styles already match the defaults: nothing to reset. */
export const isDefault = (current: Preset[], defaults: Preset[]) =>
  current.length === defaults.length && !losesWork(resetLosses(current, defaults));

/** "Proofread", "Proofread and Polish", "Proofread, Polish, and Shorter". */
export function listNames(names: string[]): string {
  const quoted = names.map((n) => `“${n}”`);
  if (quoted.length < 3) return quoted.join(" and ");
  return `${quoted.slice(0, -1).join(", ")}, and ${quoted.at(-1)}`;
}

/** The confirmation's sentence about what will be lost. */
export function describeLosses(l: ResetLosses): string {
  const parts: string[] = [];
  if (l.edited.length) parts.push(`your changes to ${listNames(l.edited)}`);
  if (l.custom.length) parts.push(`${l.custom.length === 1 ? "the style" : "the styles"} you added (${listNames(l.custom)})`);
  if (l.reordered) parts.push("your order");
  const last = parts.pop();
  return parts.length ? `${parts.join(", ")}${parts.length > 1 ? "," : ""} and ${last}` : (last ?? "");
}
