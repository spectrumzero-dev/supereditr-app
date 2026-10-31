import type { AppInfo } from "../lib/types";

/** An app SuperEditr never reads text in, with its name and icon if it's installed. */
export interface ExcludedRow {
  id: string;
  app: AppInfo | null;
}

/**
 * The excluded apps to list. `known` holds what each bundle ID's lookup found: the app, or null if
 * it isn't installed. An ID not looked up yet isn't listed, so a name never replaces a bundle ID on
 * screen. Installed apps come first, by name, then the rest by bundle ID.
 */
export function excludedRows(ids: string[], known: Record<string, AppInfo | null>): ExcludedRow[] {
  const rows = ids.filter((id) => id in known).map((id) => ({ id, app: known[id] }));
  const installed = rows.filter((row) => row.app).sort((a, b) => a.app!.name.localeCompare(b.app!.name));
  const missing = rows.filter((row) => !row.app).sort((a, b) => a.id.localeCompare(b.id));
  return [...installed, ...missing];
}

/** `list` with `ids` added, skipping blanks and IDs already there in any case, as `is_excluded` matches them. */
export function withApps(list: string[], ids: string[]): string[] {
  const result = [...list];
  for (const id of ids.map((id) => id.trim())) {
    if (id && !result.some((existing) => existing.toLowerCase() === id.toLowerCase())) result.push(id);
  }
  return result;
}
