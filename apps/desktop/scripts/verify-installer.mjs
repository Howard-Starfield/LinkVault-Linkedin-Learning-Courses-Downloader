import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";

const bundleRoot = new URL("../src-tauri/target/release/bundle/", import.meta.url);
const packageJson = JSON.parse(await readFile(new URL("../package.json", import.meta.url), "utf8"));
const candidates = [];
for (const folder of ["nsis", "msi"]) {
  try {
    for (const name of await readdir(new URL(`${folder}/`, bundleRoot))) {
      if (name.endsWith(".exe") || name.endsWith(".msi")) candidates.push(join(folder, name));
    }
  } catch {
    // One Windows bundle format may be disabled.
  }
}
const expected = join("nsis", `LinkVault_${packageJson.version}_x64-setup.exe`);
assert.ok(
  candidates.includes(expected),
  `Expected ${expected}, but found: ${candidates.join(", ") || "no Windows installers"}. Run npm run tauri build first.`
);
console.log(`Installer verification passed for ${expected}.`);
