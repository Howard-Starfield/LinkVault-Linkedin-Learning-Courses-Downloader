import { spawn, spawnSync } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, "..");
const tauriDir = path.join(root, "src-tauri");
const tauriConfigPath = path.join(tauriDir, "tauri.conf.json");
const releaseExe = path.join(tauriDir, "target", "release", "linkvault.exe");
const bundleDir = path.join(tauriDir, "target", "release", "bundle");
const nsisDir = path.join(bundleDir, "nsis");
const smokeMs = Number(process.env.LINKVAULT_RELEASE_SMOKE_MS ?? 5000);

function assertRelease(condition, message) {
  if (!condition) {
    throw new Error(`Release assertion failed: ${message}`);
  }
}

function runReleaseBuild() {
  const command = process.platform === "win32" ? "cmd.exe" : "pnpm";
  const args = process.platform === "win32" ? ["/d", "/s", "/c", "pnpm.cmd", "tauri", "build"] : ["tauri", "build"];
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: "inherit",
    shell: false,
    windowsHide: true
  });

  assertRelease(
    result.status === 0,
    `\`pnpm tauri build\` must complete successfully.${result.error ? ` ${result.error.message}` : ""}`
  );
}

async function assertBundleConfig() {
  const config = JSON.parse(await readFile(tauriConfigPath, "utf8"));
  const targets = Array.isArray(config.bundle?.targets) ? config.bundle.targets : [];
  const icons = Array.isArray(config.bundle?.icon) ? config.bundle.icon : [];

  assertRelease(config.bundle?.active === true, "Tauri bundle.active must be true.");
  assertRelease(targets.includes("nsis"), "Tauri bundle.targets must include nsis.");
  assertRelease(icons.includes("icons/icon.ico"), "Tauri bundle.icon must include icons/icon.ico.");
  assertRelease(config.bundle?.windows?.nsis?.installerIcon === "icons/icon.ico", "NSIS installerIcon must use icons/icon.ico.");
}

async function listFilesRecursive(directory) {
  if (!existsSync(directory)) return [];

  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];

  for (const entry of entries) {
    const fullPath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await listFilesRecursive(fullPath)));
    } else if (entry.isFile()) {
      files.push(fullPath);
    }
  }

  return files;
}

function formatBytes(bytes) {
  const mb = bytes / 1024 / 1024;
  return `${mb.toFixed(2)} MB`;
}

async function wait(ms) {
  await new Promise((resolve) => setTimeout(resolve, ms));
}

async function assertProcessAlive(child) {
  await wait(smokeMs);
  assertRelease(child.exitCode === null && !child.killed, "release executable exited during startup smoke window.");
}

function stopProcess(child) {
  if (!child.pid) return;

  if (process.platform === "win32") {
    spawnSync("taskkill", ["/pid", String(child.pid), "/t", "/f"], { stdio: "ignore", windowsHide: true });
    return;
  }

  child.kill("SIGTERM");
}

await assertBundleConfig();
runReleaseBuild();

assertRelease(existsSync(releaseExe), `release executable must exist at ${releaseExe}.`);

const releaseExeSize = statSync(releaseExe).size;
assertRelease(releaseExeSize > 0, "release executable must not be empty.");

const bundleFiles = await listFilesRecursive(bundleDir);
const nsisInstallers = (await listFilesRecursive(nsisDir)).filter((artifact) => {
  const fileName = path.basename(artifact).toLowerCase();
  return fileName.endsWith("-setup.exe");
});
const shareableArtifacts = [releaseExe, ...bundleFiles].filter((artifact) => {
  const extension = path.extname(artifact).toLowerCase();
  return [".exe", ".msi", ".zip"].includes(extension);
});

process.stdout.write("LinkVault release build passed.\n");
process.stdout.write(`Release executable: ${releaseExe} (${formatBytes(releaseExeSize)})\n`);

if (bundleFiles.length > 0) {
  process.stdout.write("Bundle artifacts:\n");
  for (const artifact of bundleFiles) {
    process.stdout.write(`- ${artifact} (${formatBytes(statSync(artifact).size)})\n`);
  }
} else {
  process.stdout.write("Bundle artifacts: none emitted by the current Tauri config.\n");
}

assertRelease(shareableArtifacts.length > 0, "release build must produce at least one shareable artifact.");
assertRelease(nsisInstallers.length > 0, `NSIS setup executable must exist under ${nsisDir}.`);

process.stdout.write("NSIS installers:\n");
for (const installer of nsisInstallers) {
  process.stdout.write(`- ${installer} (${formatBytes(statSync(installer).size)})\n`);
}

const app = spawn(releaseExe, [], {
  cwd: root,
  stdio: "ignore",
  windowsHide: true
});

try {
  await assertProcessAlive(app);
  process.stdout.write(`Release executable stayed alive for ${smokeMs}ms.\n`);
} finally {
  stopProcess(app);
}
