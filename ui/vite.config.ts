import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
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
      },
    },
  },
  test: { environment: "node", include: ["src/**/*.test.ts"] },
});
