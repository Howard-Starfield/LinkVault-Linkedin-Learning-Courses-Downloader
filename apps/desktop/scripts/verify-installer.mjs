import assert from "node:assert/strict";
import { readdir } from "node:fs/promises";
import { join } from "node:path";

const bundleRoot = new URL("../src-tauri/target/release/bundle/", import.meta.url);
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
assert.ok(candidates.length > 0, "No Windows installer was found. Run npm run tauri build first.");
console.log(`Installer verification passed: ${candidates.join(", ")}`);
