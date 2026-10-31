/**
 * Settings saves its whole copy of the config a moment after each change. When the app changes the
 * config in the meantime (the menu bar's "Turn off in …", the panel's "Always keep"), Settings
 * reloads, and this keeps both: each field the user changed since `base`, the config as Settings
 * last loaded or saved it, keeps the user's value, and every other field takes the app's.
 */
export function mergeUnsaved<T extends object>(base: T, unsaved: T, fresh: T): T {
  const merged = { ...fresh };
  for (const key of Object.keys(unsaved) as (keyof T)[]) {
    if (JSON.stringify(unsaved[key]) !== JSON.stringify(base[key])) merged[key] = unsaved[key];
  }
  return merged;
}
