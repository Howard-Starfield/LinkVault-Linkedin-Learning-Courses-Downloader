import { spawn, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, "..");
const tauriDir = path.join(root, "src-tauri");
const appExe = path.join(tauriDir, "target", "debug", "linkvault.exe");
const smokeMs = Number(process.env.LINKVAULT_TAURI_SMOKE_MS ?? 5000);

function assertSmoke(condition, message) {
  if (!condition) {
    throw new Error(`Tauri smoke assertion failed: ${message}`);
  }
}

async function readText(relativePath) {
  return readFile(path.join(root, relativePath), "utf8");
}

async function assertStaticWiring() {
  const packageJson = JSON.parse(await readText("package.json"));
  const capability = JSON.parse(await readText("src-tauri/capabilities/default.json"));
  const cargoToml = await readText("src-tauri/Cargo.toml");
  const libRs = await readText("src-tauri/src/lib.rs");
  const appTsx = await readText("src/App.tsx");

  assertSmoke(
    packageJson.dependencies?.["@tauri-apps/plugin-dialog"],
    "package.json must depend on @tauri-apps/plugin-dialog."
  );
  assertSmoke(
    cargoToml.includes("tauri-plugin-dialog"),
    "Cargo.toml must depend on tauri-plugin-dialog."
  );
  assertSmoke(
    libRs.includes("tauri_plugin_dialog::init()"),
    "Tauri builder must register tauri_plugin_dialog::init()."
  );
  assertSmoke(
    capability.permissions?.includes("dialog:allow-open"),
    "default capability must grant dialog:allow-open."
  );
  assertSmoke(
    appTsx.includes("@tauri-apps/plugin-dialog") && appTsx.includes("directory: true"),
    "Browse action must use the Tauri dialog plugin directory picker."
  );
}

function runTauriDebugBuild() {
  const command = process.platform === "win32" ? "cmd.exe" : "pnpm";
  const args =
    process.platform === "win32"
      ? ["/d", "/s", "/c", "pnpm.cmd", "tauri", "build", "--debug"]
      : ["tauri", "build", "--debug"];
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: "inherit",
    shell: false,
    windowsHide: true
  });

  assertSmoke(
    result.status === 0,
    `\`pnpm tauri build --debug\` must complete successfully.${result.error ? ` ${result.error.message}` : ""}`
  );
  assertSmoke(existsSync(appExe), `debug executable must exist at ${appExe}.`);
}

async function wait(ms) {
  await new Promise((resolve) => setTimeout(resolve, ms));
}

async function assertProcessAlive(child) {
  await wait(smokeMs);
  assertSmoke(child.exitCode === null && !child.killed, "LinkVault exited during startup smoke window.");
}

function stopProcess(child) {
  if (!child.pid) return;

  if (process.platform === "win32") {
    spawnSync("taskkill", ["/pid", String(child.pid), "/t", "/f"], { stdio: "ignore", windowsHide: true });
    return;
  }

  child.kill("SIGTERM");
}

await assertStaticWiring();
process.stdout.write("LinkVault Tauri static runtime wiring passed.\n");

runTauriDebugBuild();

const app = spawn(appExe, [], {
  cwd: root,
  stdio: "ignore",
  windowsHide: true
});

try {
  await assertProcessAlive(app);
  process.stdout.write(`LinkVault debug executable stayed alive for ${smokeMs}ms.\n`);
} finally {
  stopProcess(app);
}

process.stdout.write("LinkVault Tauri desktop smoke passed.\n");
