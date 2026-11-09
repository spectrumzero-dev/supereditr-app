// Tests the Omarchy bar widget's Model.js, and the manifest checks Omarchy applies before installing
// a plugin (bin/omarchy-plugin-validate). Run with: node --test packaging/omarchy/tests/model.test.mjs
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "../../..");
const Model = vm.createContext({});
vm.runInContext(readFileSync(join(here, "../Model.js"), "utf8"), Model);

test("the probe's output decides the app's state, running first", () => {
  assert.equal(Model.appState("installed\nrunning\n"), "running");
  assert.equal(Model.appState("running\n"), "running");
  assert.equal(Model.appState("installed\n"), "stopped");
  assert.equal(Model.appState(""), "missing");
});

test("the status line names the running version", () => {
  const version = Model.version('{"accessibility":true,"version":"0.1.6","pid":4242,"updated":1}');
  assert.equal(Model.statusLine("running", version), "Running · version 0.1.6");
  assert.equal(Model.statusLine("running", Model.version("not json")), "Running");
  assert.equal(Model.statusLine("stopped", version), "Not running");
  assert.equal(Model.statusLine("missing", ""), "Not installed");
});

test("shortcuts come from linux_hotkeys, never the Mac's", () => {
  const settings = JSON.stringify({
    hotkeys: { review: "Cmd+Shift+E", quick_fix: "Cmd+Alt+E" },
    linux_hotkeys: { review: "Super+Shift+R", quick_fix: "" },
  });
  const shortcuts = Model.shortcuts(settings);
  assert.equal(shortcuts.review, "Super+Shift+R");
  assert.equal(Model.shortcutText(shortcuts.quickFix), "Off");
});

test("missing or unreadable settings mean the defaults", () => {
  for (const text of ["", "{", JSON.stringify({ hotkeys: { review: "Cmd+E" } }), JSON.stringify({ linux_hotkeys: null })]) {
    const shortcuts = Model.shortcuts(text);
    assert.equal(shortcuts.review, "Super+Alt+E", text);
    assert.equal(shortcuts.quickFix, "Super+Control+Alt+E", text);
  }
  assert.equal(Model.shortcuts(JSON.stringify({ linux_hotkeys: { review: "Super+R" } })).quickFix, "Super+Control+Alt+E");
});

test("every icon is a single character", () => {
  for (const name of Object.keys(Model.GLYPHS)) assert.equal([...Model.glyph(name)].length, 1, name);
});

test("the manifest passes Omarchy's checks", () => {
  const manifest = JSON.parse(readFileSync(join(root, "manifest.json"), "utf8"));
  assert.equal(manifest.schemaVersion, 1);
  for (const field of ["id", "name", "version", "author", "description", "kinds", "entryPoints"]) {
    assert.ok(manifest[field], `manifest needs ${field}`);
  }
  assert.match(manifest.id, /^[A-Za-z0-9][A-Za-z0-9._-]*$/);
  assert.ok(!manifest.id.startsWith("omarchy."), "omarchy.* is reserved");
  assert.deepEqual(manifest.kinds, ["bar-widget"]);
  const entry = manifest.entryPoints.barWidget;
  assert.ok(entry && !entry.startsWith("/") && !entry.includes(".."), "a safe relative entry point");
  assert.ok(existsSync(join(root, entry)), `${entry} exists`);
  assert.ok(["left", "centerBegin", "center", "centerEnd", "tray", "right"].includes(manifest.barWidget.defaultSection));
  // The widget registers under the plugin's id.
  assert.match(readFileSync(join(root, entry), "utf8"), new RegExp(`moduleName: "${manifest.id.replaceAll(".", "\\.")}"`));
});
