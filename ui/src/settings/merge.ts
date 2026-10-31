/**
 * Settings saves its whole copy of the config a moment after each change. When the app changes the
 * config in the meantime (the menu bar's "Turn off in …", the panel's "Always keep"), Settings
 * reloads, and this keeps both: each field the user changed since `base`, the config as Settings
 * last loaded or saved it, keeps the user's value, and every other field takes the app's. A save
 * already on its way is refused by the app (see `save_settings` in commands.rs) and made again.
 */
export function mergeUnsaved<T extends object>(base: T, unsaved: T, fresh: T): T {
  const merged = { ...fresh };
  for (const key of Object.keys(unsaved) as (keyof T)[]) {
    if (JSON.stringify(unsaved[key]) !== JSON.stringify(base[key])) merged[key] = unsaved[key];
  }
  return merged;
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
