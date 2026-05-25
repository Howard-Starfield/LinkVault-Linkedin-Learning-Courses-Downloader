import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { createReadStream, existsSync, statSync } from "node:fs";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, "..");
const tauriDir = path.join(root, "src-tauri");
const outputDir = path.join(root, "output", "release");
const manifestPath = path.join(outputDir, "linkvault-release-manifest.json");
const releaseExe = path.join(tauriDir, "target", "release", "linkvault.exe");
const nsisDir = path.join(tauriDir, "target", "release", "bundle", "nsis");
const configPath = path.join(tauriDir, "tauri.conf.json");

function assertManifest(condition, message) {
  if (!condition) {
    throw new Error(`Release manifest assertion failed: ${message}`);
  }
}

function gitValue(args) {
  const result = spawnSync("git", args, {
    cwd: root,
    encoding: "utf8",
    windowsHide: true
  });

  return result.status === 0 ? result.stdout.trim() : null;
}

async function sha256(filePath) {
  const hash = createHash("sha256");
  await new Promise((resolve, reject) => {
    const stream = createReadStream(filePath);
    stream.on("data", (chunk) => hash.update(chunk));
    stream.on("error", reject);
    stream.on("end", resolve);
  });
  return hash.digest("hex");
}

async function listNsisInstallers() {
  if (!existsSync(nsisDir)) return [];

  const entries = await readdir(nsisDir, { withFileTypes: true });
  return entries
    .filter((entry) => entry.isFile() && entry.name.toLowerCase().endsWith("-setup.exe"))
    .map((entry) => path.join(nsisDir, entry.name))
    .sort();
}

async function listUpdaterSignatures() {
  if (!existsSync(nsisDir)) return [];

  const entries = await readdir(nsisDir, { withFileTypes: true });
  return entries
    .filter((entry) => entry.isFile() && entry.name.toLowerCase().endsWith(".sig"))
    .map((entry) => path.join(nsisDir, entry.name))
    .sort();
}

function artifactRecord(filePath, kind) {
  const stats = statSync(filePath);
  return {
    kind,
    name: path.basename(filePath),
    path: path.relative(root, filePath).replaceAll(path.sep, "/"),
    sizeBytes: stats.size
  };
}

const config = JSON.parse(await readFile(configPath, "utf8"));
const expectedPrefix = `${config.productName}_${config.version}_`;
const installers = (await listNsisInstallers()).filter((installer) => path.basename(installer).startsWith(expectedPrefix));
const signatures = (await listUpdaterSignatures()).filter((signature) => path.basename(signature).startsWith(expectedPrefix));

assertManifest(existsSync(releaseExe), `release executable missing at ${releaseExe}; run pnpm.cmd run verify:release first.`);
assertManifest(installers.length > 0, `NSIS installer missing under ${nsisDir}; run pnpm.cmd run verify:release first.`);
assertManifest(signatures.length > 0, `Updater signature missing under ${nsisDir}; run pnpm.cmd run verify:release first.`);

const artifacts = [
  artifactRecord(releaseExe, "release-exe"),
  ...installers.map((installer) => artifactRecord(installer, "nsis-installer")),
  ...signatures.map((signature) => artifactRecord(signature, "updater-signature"))
];

for (const artifact of artifacts) {
  const absolutePath = path.join(root, artifact.path);
  artifact.sha256 = await sha256(absolutePath);
  assertManifest(/^[a-f0-9]{64}$/.test(artifact.sha256), `${artifact.name} should have a SHA-256 hash.`);
  assertManifest(artifact.sizeBytes > 0, `${artifact.name} should not be empty.`);
}

const manifest = {
  productName: config.productName,
  version: config.version,
  identifier: config.identifier,
  bundleTargets: config.bundle?.targets ?? [],
  generatedAtUtc: new Date().toISOString(),
  gitCommit: gitValue(["rev-parse", "--short", "HEAD"]),
  gitDirty: Boolean(gitValue(["status", "--short"])),
  artifacts
};

assertManifest(manifest.productName === "LinkVault", `expected productName LinkVault, saw ${manifest.productName}.`);
assertManifest(manifest.bundleTargets.includes("nsis"), "manifest should include nsis bundle target.");
assertManifest(config.bundle?.createUpdaterArtifacts === true, "release config should create updater artifacts.");

await mkdir(outputDir, { recursive: true });
await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");

process.stdout.write(`LinkVault release manifest written to ${manifestPath}\n`);
for (const artifact of manifest.artifacts) {
  process.stdout.write(`- ${artifact.kind}: ${artifact.name} sha256=${artifact.sha256}\n`);
}
