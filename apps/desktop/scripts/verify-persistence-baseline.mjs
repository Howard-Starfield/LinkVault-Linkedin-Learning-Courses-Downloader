import { spawnSync } from "node:child_process";
import process from "node:process";

const result = spawnSync(
  "cargo",
  [
    "test",
    "--release",
    "--manifest-path",
    "./src-tauri/Cargo.toml",
    "persistence_release_",
    "--",
    "--ignored",
    "--nocapture",
    "--test-threads=1",
  ],
  {
    cwd: new URL("..", import.meta.url),
    encoding: "utf8",
    shell: process.platform === "win32",
    env: { ...process.env, CARGO_BUILD_JOBS: "1" },
  },
);

process.stdout.write(result.stdout ?? "");
process.stderr.write(result.stderr ?? "");
if (result.status !== 0) {
  process.exit(result.status ?? 1);
}

const marker = "LINKVAULT_PERSISTENCE_BASELINE=";
const line = (result.stdout ?? "")
  .split(/\r?\n/)
  .find((candidate) => candidate.includes(marker));
if (!line) {
  throw new Error("Persistence baseline did not emit its structured report");
}

const report = JSON.parse(line.slice(line.indexOf(marker) + marker.length));
if (
  report.acceptedWrites !== 800 ||
  report.completedWrites !== 800 ||
  report.failedWrites !== 0 ||
  report.snapshotRows !== 800 ||
  report.contentionElapsedMs >= 5_000 ||
  report.snapshotReadElapsedMs >= 250
) {
  throw new Error(
    `Persistence baseline missed its acceptance thresholds: ${JSON.stringify(report)}`,
  );
}

const diagnosticsMarker = "LINKVAULT_PERSISTENCE_DIAGNOSTICS=";
const diagnosticsLine = (result.stdout ?? "")
  .split(/\r?\n/)
  .find((candidate) => candidate.includes(diagnosticsMarker));
if (!diagnosticsLine) {
  throw new Error("Persistence baseline did not emit a diagnostic sample");
}
const diagnosticSample = JSON.parse(
  diagnosticsLine.slice(
    diagnosticsLine.indexOf(diagnosticsMarker) + diagnosticsMarker.length,
  ),
);
const serializedDiagnostics = JSON.stringify(diagnosticSample).toLowerCase();
for (const forbidden of [
  "message",
  "payload",
  "cookie",
  "authorization",
  "token",
]) {
  if (serializedDiagnostics.includes(forbidden)) {
    throw new Error(`Diagnostic sample contains forbidden field ${forbidden}`);
  }
}

console.log(`Persistence release baseline passed: ${JSON.stringify(report)}`);
console.log(
  `Persistence diagnostic sample passed: ${JSON.stringify(diagnosticSample)}`,
);
