import { existsSync, statSync } from "node:fs";
import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, "..");
const tauriDir = path.join(root, "src-tauri");
const configPath = path.join(tauriDir, "tauri.conf.json");
const nsisDir = path.join(tauriDir, "target", "release", "bundle", "nsis");

function assertInstaller(condition, message) {
  if (!condition) {
    throw new Error(`Installer assertion failed: ${message}`);
  }
}

function formatBytes(bytes) {
  const mb = bytes / 1024 / 1024;
  return `${mb.toFixed(2)} MB`;
}

async function listNsisInstallers() {
  if (!existsSync(nsisDir)) return [];

  const entries = await readdir(nsisDir, { withFileTypes: true });
  return entries
    .filter((entry) => entry.isFile() && entry.name.toLowerCase().endsWith("-setup.exe"))
    .map((entry) => path.join(nsisDir, entry.name))
    .sort();
}

const config = JSON.parse(await readFile(configPath, "utf8"));
const productName = config.productName;
const version = config.version;
const expectedPrefix = `${productName}_${version}_`;

assertInstaller(productName === "LinkVault", `expected productName LinkVault, saw ${productName}.`);
assertInstaller(version === "0.1.0", `expected version 0.1.0, saw ${version}.`);

const installers = await listNsisInstallers();
assertInstaller(
  installers.length > 0,
  `expected at least one NSIS setup executable under ${nsisDir}; run pnpm.cmd run verify:release first.`
);

for (const installer of installers) {
  const fileName = path.basename(installer);
  const stats = statSync(installer);
  const header = await readFile(installer, { encoding: null, flag: "r" }).then((buffer) => buffer.subarray(0, 2).toString("ascii"));

  assertInstaller(fileName.startsWith(expectedPrefix), `${fileName} should start with ${expectedPrefix}.`);
  assertInstaller(fileName.endsWith("-setup.exe"), `${fileName} should be a setup executable.`);
  assertInstaller(stats.size > 1024 * 1024, `${fileName} should be larger than 1 MB, saw ${formatBytes(stats.size)}.`);
  assertInstaller(header === "MZ", `${fileName} should have a Windows PE MZ header.`);

  process.stdout.write(`Verified NSIS installer: ${installer} (${formatBytes(stats.size)})\n`);
}

process.stdout.write("LinkVault installer artifact assertions passed.\n");
