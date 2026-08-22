import { lstat, readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  BINARY_RELATIVE_PATH,
  fail,
  REQUIRED_COMPONENTS,
  readLock,
  resolveInside,
  sha256File,
} from "./youtube-helper-lock.mjs";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const desktopDirectory = path.resolve(scriptDirectory, "..");
const repositoryRoot = path.resolve(desktopDirectory, "../..");

async function verifySidecarConfig() {
  const configPath = path.join(desktopDirectory, "src-tauri", "tauri.youtube.conf.json");
  const config = JSON.parse(await readFile(configPath, "utf8"));
  const actual = config?.bundle?.externalBin;
  const expected = REQUIRED_COMPONENTS.map((name) => `binaries/${name}`);
  if (!Array.isArray(actual) || JSON.stringify(actual) !== JSON.stringify(expected)) {
    fail(`tauri.youtube.conf.json externalBin must be exactly ${JSON.stringify(expected)}`);
  }
}

function filePath(relativePath) {
  return resolveInside(repositoryRoot, relativePath, "installed helper path");
}

async function verifyInstalledAsset(asset, label) {
  const relativePath = asset.path ?? asset.filename;
  const targetPath = filePath(path.join(BINARY_RELATIVE_PATH, relativePath));
  let entry;
  try {
    entry = await lstat(targetPath);
  } catch {
    fail(`${label} is missing at ${path.relative(repositoryRoot, targetPath)}`);
  }
  if (!entry.isFile() || entry.isSymbolicLink()) fail(`${label} is not a regular non-link file`);
  if (entry.size !== asset.sizeBytes) fail(`${label} size mismatch: expected ${asset.sizeBytes}, got ${entry.size}`);
  const actualHash = await sha256File(targetPath);
  if (actualHash !== asset.sha256) fail(`${label} SHA-256 mismatch: expected ${asset.sha256}, got ${actualHash}`);
}

async function verifyLicense(asset, label) {
  const licensePath = filePath(asset.licenseFile);
  try {
    const license = await readFile(licensePath, "utf8");
    if (license.trim().length === 0) fail(`${label}.licenseFile is empty`);
  } catch (error) {
    fail(`${label}.licenseFile is unavailable: ${error.message}`);
  }
}

await verifySidecarConfig();
const { lock, validation } = await readLock(repositoryRoot);
if (!validation.populated) {
  if (lock.status === "unpopulated") {
    fail("lock is intentionally unpopulated; helper execution and packaging remain blocked until authoritative metadata is reviewed");
  }
  fail(`lock status ${lock.status} is non-executable; helper execution and packaging require status ready`);
}

for (const component of lock.components) {
  await verifyInstalledAsset(component, component.name);
  await verifyLicense(component, component.name);
  for (const [index, asset] of component.loadedAssets.entries()) {
    await verifyInstalledAsset(asset, `${component.name}.loadedAssets[${index}]`);
    await verifyLicense(asset, `${component.name}.loadedAssets[${index}]`);
  }
}

console.log(`YouTube helper verification passed for lock ${lock.lockDigest}.`);
