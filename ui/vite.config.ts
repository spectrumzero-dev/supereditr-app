import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { svelteTesting } from "@testing-library/svelte/vite";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import type { Plugin } from "vite";

// Fails the build when a module from node_modules goes into the bundle and its package is missing
// from the third-party licences in Settings (scripts/third-party-notices.py writes that list).
// It checks every module the build loads, including those whose code is later tree-shaken.
function noticesListBundledPackages(): Plugin {
  return {
    name: "notices-list-bundled-packages",
    apply: "build",
    buildEnd(error) {
      if (error) return;
      const notices = JSON.parse(readFileSync(resolve(import.meta.dirname, "src/settings/third-party-notices.json"), "utf8")) as {
        packages: { kind: string; name: string }[];
      };
      const listed = new Set(notices.packages.filter((p) => p.kind === "js").map((p) => p.name));
      const missing = new Set<string>();
      for (const id of this.getModuleIds()) {
        const parts = id.split(/[\\/]node_modules[\\/]/);
        if (parts.length < 2) continue;
        const path = parts[parts.length - 1].split(/[\\/]/);
        const name = path[0].startsWith("@") ? `${path[0]}/${path[1]}` : path[0];
        if (!listed.has(name)) missing.add(name);
      }
      if (missing.size) {
        this.error(`The bundle includes ${[...missing].sort().join(", ")}, missing from the third-party licences. List it in scripts/third-party-notices.py and run python3 -I scripts/third-party-notices.py.`);
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
