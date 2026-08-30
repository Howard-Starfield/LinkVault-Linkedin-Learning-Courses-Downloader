import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const desktop = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const styles = await readFile(path.join(desktop, "src/index.css"), "utf8");
const packageText = await readFile(path.join(desktop, "package.json"), "utf8");
const rootPackageText = await readFile(path.join(desktop, "../../package.json"), "utf8");
const packageJson = JSON.parse(packageText);
const rootPackageJson = JSON.parse(rootPackageText);

function includes(source, fragment, message) {
  assert.ok(source.includes(fragment), message ?? `Missing ${fragment}`);
}

includes(
  styles,
  "grid-template-columns: minmax(0, 1fr) auto;",
  "Artifact rows must use two-column grid"
);
assert.match(
  styles,
  /\.linkedin-artifact-row[\s\S]*?display:\s*grid[\s\S]*?grid-template-columns:\s*minmax\(0,\s*1fr\)\s*auto;/,
  "LinkedIn artifact row must use the two-column grid"
);
assert.match(
  styles,
  /\.coursera-artifact-row[\s\S]*?display:\s*grid[\s\S]*?grid-template-columns:\s*minmax\(0,\s*1fr\)\s*auto;/,
  "Coursera artifact row must use the two-column grid"
);
includes(styles, ".linkedin-artifact-toggles {", "LinkedIn artifact toggles rule is missing");
includes(styles, ".coursera-artifact-toggles {", "Coursera artifact toggles rule is missing");
includes(styles, ".linkedin-primary-actions {", "LinkedIn primary actions rule is missing");
includes(styles, ".coursera-primary-actions {", "Coursera primary actions rule is missing");
assert.match(
  styles,
  /\.linkedin-artifact-row[\s\S]*?display:\s*grid[\s\S]*?min-width:\s*0;/,
  "LinkedIn artifact row must be a min-width zero grid"
);
assert.match(
  styles,
  /@container lv-main \(max-width: 720px\)[\s\S]*\.linkedin-artifact-row[\s\S]*grid-template-columns:\s*1fr|flex-direction:\s*column/,
  "720px container must still stack LinkedIn artifact rows"
);

assert.equal(
  packageJson.scripts["verify:linkedin-artifact-row"],
  "node ./scripts/verify-linkedin-artifact-row.mjs",
  "Desktop LinkedIn artifact-row verifier is not wired"
);
assert.equal(
  rootPackageJson.scripts["verify:linkedin-artifact-row"],
  "npm --prefix apps/desktop run verify:linkedin-artifact-row",
  "Root LinkedIn artifact-row verifier is not wired"
);

console.log("LinkedIn and Coursera artifact-row grid contracts passed.");
