import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { svelteTesting } from "@testing-library/svelte/vite";
import { resolve } from "node:path";

export default defineConfig({
  plugins: [svelte()],
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
