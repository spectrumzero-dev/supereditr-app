// Renders the SVG sources in ui/assets to the PNGs the app bundle needs.
// The app icon set (icns/ico/png sizes) is then generated with `tauri icon`.
import { Resvg } from "@resvg/resvg-js";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const render = (svg, width, out) => {
  const png = new Resvg(readFileSync(join(root, "assets", svg)), { fitTo: { mode: "width", value: width } }).render().asPng();
  writeFileSync(out, png);
  console.log(`${out} (${width}px)`);
};
render("app-icon.svg", 1024, join(root, "assets", "app-icon.png"));
render("tray.svg", 36, join(root, "..", "src-tauri", "icons", "tray.png"));
render("app-icon.svg", 256, join(root, "public", "icon.png"));
