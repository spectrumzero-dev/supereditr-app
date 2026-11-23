import { describe, expect, it } from "vitest";
import { packageRoot, unlistedPackages, type PackageVersion } from "./bundled-packages";

const installed: Record<string, PackageVersion> = {
  "/app/node_modules/clsx": { name: "clsx", version: "2.1.1" },
  "/app/node_modules/svelte": { name: "svelte", version: "5.57.2" },
  "/app/node_modules/svelte/node_modules/clsx": { name: "clsx", version: "99.0.0" },
};
const read = (root: string) => installed[root];
const listed = [
  { name: "clsx", version: "2.1.1" },
  { name: "svelte", version: "5.57.2" },
];

describe("unlistedPackages", () => {
  it("passes packages listed at the installed version", () => {
    const ids = ["/app/src/main.ts", "/app/node_modules/clsx/dist/clsx.mjs", "/app/node_modules/svelte/src/index-client.js"];
    expect(unlistedPackages(ids, listed, read)).toEqual([]);
  });

  it("reports a second version of a listed package", () => {
    const ids = ["/app/node_modules/clsx/dist/clsx.mjs", "/app/node_modules/svelte/node_modules/clsx/dist/clsx.mjs"];
    expect(unlistedPackages(ids, listed, read)).toEqual(["clsx@99.0.0"]);
  });

  it("reports a package that is not listed", () => {
    expect(unlistedPackages(["/app/node_modules/svelte/src/index-client.js"], [], read)).toEqual(["svelte@5.57.2"]);
  });
});

describe("packageRoot", () => {
  it("finds the package of a file, nested, scoped or on Windows", () => {
    expect(packageRoot("/app/node_modules/clsx/dist/clsx.mjs")).toBe("/app/node_modules/clsx");
    expect(packageRoot("/app/node_modules/svelte/node_modules/clsx/dist/clsx.mjs")).toBe("/app/node_modules/svelte/node_modules/clsx");
    expect(packageRoot("/app/node_modules/@tauri-apps/api/core.js")).toBe("/app/node_modules/@tauri-apps/api");
    expect(packageRoot("C:\\app\\node_modules\\clsx\\dist\\clsx.mjs")).toBe("C:/app/node_modules/clsx");
  });

  it("ignores node_modules in a query", () => {
    expect(packageRoot("/app/src/App.svelte?svelte&type=style&lang.css&x=/node_modules/evil/a.js")).toBeNull();
    expect(packageRoot("/app/node_modules/clsx/dist/clsx.mjs?x=/node_modules/evil/a.js")).toBe("/app/node_modules/clsx");
  });

  it("ignores virtual modules and modules of the app", () => {
    expect(packageRoot("\0vite/preload-helper.js")).toBeNull();
    expect(packageRoot("/app/src/settings/App.svelte")).toBeNull();
  });
});
