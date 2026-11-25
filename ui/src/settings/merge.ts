import type { Config, SettingsPayload } from "../lib/types";
import { appIdentity } from "./excluded";

type Mergeable = Pick<Config, "excluded_apps" | "words" | "providers" | "active_provider">;

/** Matches the dictionary's per-character case folding and whitespace trimming in Rust. */
export function wordIdentity(word: string): string {
  const trimmed = word.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "");
  return Array.from(trimmed, (char) => Array.from(char.toLowerCase())[0]).join("");
}

/** During recovery a missing entry may be unreadable, so its absence can't settle a pending edit. */
export function needsRepairBeforeMerge(base: Config, unsaved: Config, fresh: SettingsPayload): boolean {
  const recovery = fresh.recovery;
  if (!recovery) return false;
  if (!recovery.writable) return true;
  if (!recovery.exclusionsSafe && !sameValue(base.excluded_apps, unsaved.excluded_apps)) return true;
  if (!recovery.issues.includes("providers")) return false;

  const changedChoice = unsaved.active_provider !== base.active_provider;
  return base.providers.some((old) => {
    const edit = unsaved.providers.find((p) => p.id === old.id);
    const affected = !sameValue(old, edit) || (changedChoice && unsaved.active_provider === old.id);
    return affected && !fresh.config.providers.some((p) => p.id === old.id && p.kind === old.kind);
  });
}

/**
 * Settings saves its whole copy of the config a moment after each change. When the app changes the
 * config in the meantime (the menu bar's "Turn off in …", the panel's "Always keep"), Settings
 * reloads, and this keeps both: each field the user changed since `base`, the config as Settings
 * last loaded or saved it, keeps the user's value, and every other field takes the app's. A save
 * already on its way is refused by the app (see `save_settings` in commands.rs) and made again.
 */
export function mergeUnsaved<T extends Partial<Mergeable>>(base: T, unsaved: T, fresh: T): T {
  const merged = mergeFields(base, unsaved, fresh);
  if (base.excluded_apps && unsaved.excluded_apps && fresh.excluded_apps) {
    merged.excluded_apps = mergeEntries(base.excluded_apps, unsaved.excluded_apps, fresh.excluded_apps, appIdentity, (_old, _edit, current) => current);
  }
  if (base.words && unsaved.words && fresh.words) {
    merged.words = mergeEntries(base.words, unsaved.words, fresh.words, wordIdentity, (old, edit, current) => old === edit ? current : edit);
  }
  if (base.providers && unsaved.providers && fresh.providers) {
    merged.providers = mergeEntries(base.providers, unsaved.providers, fresh.providers, (p) => p.id, (old, edit, current) => {
      // A profile replaced with another provider kind has different connection settings.
      if (old.kind !== current.kind) return current;
      if (old.kind !== edit.kind) return edit;
      return mergeFields(old, edit, current);
    });
    if (merged.active_provider && unsaved.providers.some((p) => p.id === merged.active_provider)
      && !merged.providers.some((p) => p.id === merged.active_provider)
      && merged.active_provider !== fresh.active_provider) {
      merged.active_provider = fresh.active_provider;
    }
  }
  return merged;
}

function mergeFields<T extends object>(base: T, unsaved: T, fresh: T): T {
  const merged = { ...fresh };
  for (const key of Object.keys({ ...base, ...unsaved }) as (keyof T)[]) {
    if (sameValue(base[key], unsaved[key])) continue;
    if (key in unsaved) merged[key] = unsaved[key];
    else delete merged[key];
  }
  return merged;
}

function sameValue(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

/** Apply both sides' additions and removals relative to the list they started from. */
function mergeEntries<T>(
  base: T[], unsaved: T[], fresh: T[], identity: (entry: T) => string,
  merge: (old: T, edit: T, current: T) => T,
): T[] {
  if (sameValue(base, unsaved)) return fresh;
  const byId = (list: T[]) => new Map(list.map((entry) => [identity(entry), entry]));
  const original = byId(base);
  const edited = byId(unsaved);
  const result = new Map<string, T>();
  for (const entry of fresh) {
    const id = identity(entry);
    const old = original.get(id);
    const edit = edited.get(id);
    if (old === undefined) result.set(id, entry);
    else if (edit !== undefined) result.set(id, merge(old, edit, entry));
  }
  for (const [id, edit] of edited) {
    if (!original.has(id) && !result.has(id)) result.set(id, edit);
  }
  return [...result.values()];
}

/**
 * Runs `save`, which returns false when the app refused it for being based on settings it has saved
 * since. Then reloads, which merges the unsaved changes into the app's settings, and tries again, at
 * most `attempts` times in all. Returns whether a save went through.
 */
export async function saveUntilCurrent(save: () => Promise<boolean>, reload: () => Promise<void>, attempts = 3): Promise<boolean> {
  for (let attempt = 1; attempt <= attempts; attempt++) {
    if (await save()) return true;
    if (attempt < attempts) await reload();
  }
  return false;
}
