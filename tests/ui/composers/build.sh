#!/bin/bash
# Rebuilds composers.bundle.js, the editors the UI smoke tests' composer matrix runs in Chrome, and
# LICENSES.txt, the licence of every package in it. Both are committed, so the smoke tests never
# fetch anything. Run after changing src/composers.js or a version in package.json.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
npm ci --ignore-scripts --no-audit --no-fund
mkdir -p build
npx esbuild src/composers.js --bundle --format=iife --minify --legal-comments=none --loader:.css=text --conditions=production \
  --define:process.env.NODE_ENV='"production"' --metafile=build/meta.json --outfile=composers.bundle.js
# Each package the bundle includes, with its version and licence text.
node -e '
const fs = require("fs"), path = require("path");
const inputs = Object.keys(JSON.parse(fs.readFileSync("build/meta.json")).inputs);
const dirs = [...new Set(inputs.map((i) => i.match(/^(.*node_modules\/(?:@[^/]+\/)?[^/]+)/)?.[1]).filter(Boolean))].sort();
const out = ["Packages bundled into composers.bundle.js, with their licences.", ""];
for (const dir of dirs) {
  const pkg = JSON.parse(fs.readFileSync(path.join(dir, "package.json")));
  // The Slate packages ship without a licence file; slate-License.md is the one in their repository.
  const file = fs.readdirSync(dir).find((f) => /^(licen[cs]e|copying)/i.test(f));
  const licence = file ? path.join(dir, file) : pkg.name.startsWith("slate") ? "slate-License.md" : null;
  if (!licence) throw new Error(`${pkg.name} has no licence file`);
  out.push(`${"=".repeat(72)}`, `${pkg.name} ${pkg.version} (${pkg.license})`, "", fs.readFileSync(licence, "utf8").trim(), "");
}
fs.writeFileSync("LICENSES.txt", out.join("\n"));
console.log(`${dirs.length} packages: ${dirs.map((d) => d.replace(/.*node_modules\//, "")).join(", ")}`);
'
