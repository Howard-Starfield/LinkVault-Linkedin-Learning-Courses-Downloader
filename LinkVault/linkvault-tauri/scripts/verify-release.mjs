import { spawn, spawnSync } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import { readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, "..");
const tauriDir = path.join(root, "src-tauri");
const releaseExe = path.join(tauriDir, "target", "release", "linkvault.exe");
const bundleDir = path.join(tauriDir, "target", "release", "bundle");
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

runReleaseBuild();

assertRelease(existsSync(releaseExe), `release executable must exist at ${releaseExe}.`);

const releaseExeSize = statSync(releaseExe).size;
assertRelease(releaseExeSize > 0, "release executable must not be empty.");

const bundleFiles = await listFilesRecursive(bundleDir);
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
