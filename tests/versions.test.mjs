// Every file that carries SuperEditr's version carries the same one, so a version names the same
// code on macOS, Linux and Windows, and the Omarchy bar widget shares it (docs/versioning.md).
// Run with: node --test tests/versions.test.mjs
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path) => readFileSync(join(repo, path), "utf8");
const json = (path) => JSON.parse(read(path));

test("every version file names the app's version", () => {
  const cargo = read("Cargo.toml").match(/^\[workspace\.package\]\n(?:[^[\n].*\n)*?version = "([^"]+)"/m);
  assert.ok(cargo, "Cargo.toml's [workspace.package] has a version");
  const version = cargo[1];
  const lock = json("ui/package-lock.json");
  const found = {
    "src-tauri/tauri.conf.json": json("src-tauri/tauri.conf.json").version,
    "ui/package.json": json("ui/package.json").version,
    "ui/package-lock.json": lock.version,
    'ui/package-lock.json packages[""]': lock.packages[""].version,
    "packaging/omarchy/manifest.json": json("packaging/omarchy/manifest.json").version,
  };
  // The workspace's own crates are the Cargo.lock packages without a source.
  for (const block of read("Cargo.lock").split("\n[[package]]\n").slice(1)) {
    if (/^source = /m.test(block)) continue;
    const [, name] = block.match(/^name = "([^"]+)"/m);
    found[`Cargo.lock ${name}`] = block.match(/^version = "([^"]+)"/m)[1];
  }
  for (const [file, value] of Object.entries(found)) assert.equal(value, version, `${file} is ${value}, not Cargo.toml's ${version}`);
});
