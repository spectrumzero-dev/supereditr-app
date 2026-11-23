// Used by vite.config.ts when building: finds the packages in node_modules that the build's modules
// come from, and those the third-party licences in Settings do not list at that version.

export type PackageVersion = { name: string; version: string };

/** The folder of the installed package a module comes from: the path up to the package name after
 * the last node_modules segment, so a copy nested inside another package counts as its own. Null for
 * virtual modules and for modules outside node_modules. */
export function packageRoot(id: string): string | null {
  if (id.startsWith("\0")) return null;
  // A query can contain a path of its own, so it goes before the path is split.
  const parts = id.split("?")[0].split(/[\\/]/);
  const at = parts.lastIndexOf("node_modules");
  if (at < 0) return null;
  const end = at + (parts[at + 1]?.startsWith("@") ? 3 : 2);
  return end <= parts.length ? parts.slice(0, end).join("/") : null;
}

/** Each package (as name@version) that a module in `ids` comes from and `listed` lacks.
 * `readPackage` reads the name and version from a package folder's package.json. */
export function unlistedPackages(
  ids: Iterable<string>,
  listed: PackageVersion[],
  readPackage: (root: string) => PackageVersion,
): string[] {
  const known = new Set(listed.map(({ name, version }) => `${name}@${version}`));
  const roots = new Set<string>();
  for (const id of ids) {
    const root = packageRoot(id);
    if (root) roots.add(root);
  }
  const missing = new Set<string>();
  for (const root of roots) {
    const { name, version } = readPackage(root);
    if (!known.has(`${name}@${version}`)) missing.add(`${name}@${version}`);
  }
  return [...missing].sort();
}
