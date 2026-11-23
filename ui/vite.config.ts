import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { svelteTesting } from "@testing-library/svelte/vite";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import type { Plugin } from "vite";
import { unlistedPackages, type PackageVersion } from "./src/lib/bundled-packages";

// Fails the build when a module from node_modules goes into the bundle and its package, at that
// version, is missing from the third-party licences in Settings (scripts/third-party-notices.py
// writes that list). It checks the module graph, so a package whose code is later tree-shaken counts.
// Virtual modules, such as Vite's own helpers, have no package and are listed in the script instead.
function noticesListBundledPackages(): Plugin {
  return {
    name: "notices-list-bundled-packages",
    apply: "build",
    buildEnd(error) {
      if (error) return;
      const notices = JSON.parse(readFileSync(resolve(import.meta.dirname, "src/settings/third-party-notices.json"), "utf8")) as {
        packages: { kind: string; name: string; version: string }[];
      };
      const listed = notices.packages.filter((p) => p.kind === "js");
      const missing = unlistedPackages(this.getModuleIds(), listed, (root) => {
        const { name, version } = JSON.parse(readFileSync(`${root}/package.json`, "utf8")) as PackageVersion;
        return { name, version };
      });
      if (missing.length) {
        this.error(
          `The build's module graph includes ${missing.join(", ")}, missing from the third-party licences at that version.` +
            " A package whose code is tree-shaken still counts. List it in scripts/third-party-notices.py (BUNDLED_JS)" +
            " and run python3 -I scripts/third-party-notices.py; a copy nested inside another package is not listed by the script yet.",
        );
      }
    },
  };
}

export default defineConfig({
  plugins: [svelte(), noticesListBundledPackages()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: process.env.TAURI_DEV_HOST || false },
  build: {
    target: "safari16",
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: {
      input: {
        overlay: resolve(import.meta.dirname, "overlay.html"),
        settings: resolve(import.meta.dirname, "settings.html"),
        guide: resolve(import.meta.dirname, "guide.html"),
        update: resolve(import.meta.dirname, "update.html"),
      },
    },
  },
  test: {
    projects: [
      { extends: true, test: { name: "unit", environment: "node", include: ["src/**/*.test.ts"], exclude: ["src/**/*.svelte.test.ts"] } },
      // Component tests mount Svelte components in a simulated DOM, with Svelte's browser build and
      // the components' styles, so a key hidden with CSS counts as hidden.
      { extends: true, plugins: [svelteTesting()], test: { name: "component", environment: "jsdom", css: true, include: ["src/**/*.svelte.test.ts"] } },
    ],
  },
});
