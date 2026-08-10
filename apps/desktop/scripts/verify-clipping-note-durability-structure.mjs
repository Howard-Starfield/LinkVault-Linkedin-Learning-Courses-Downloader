import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const desktop = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const source = (relative) => readFile(path.join(desktop, relative), "utf8");
const lines = (text) => text.replace(/\r?\n$/, "").split(/\r?\n/).length;
const budgets = new Map([
  ["src/App.tsx", 4444],
  ["src/components/newspaper/clipping-note-save-controller.ts", 340],
  ["src/components/newspaper/clipping-note-checkpoint-controller.ts", 300],
  ["src/components/newspaper/clipping-note-durability-api.ts", 200],
  ["src/components/newspaper/useClippingNoteDurability.ts", 260],
  ["src/components/newspaper/useClippingNoteExitBridge.ts", 260],
  ["src/components/newspaper/NewspaperClippingDetail.tsx", 260],
  ["src/components/newspaper/NewspaperClippings.tsx", 140],
  ["src/components/newspaper/ClippingNoteEditor.tsx", 500],
  ["src/components/newspaper/clipping-note-slash-command.tsx", 300],
  ["scripts/clipping-note-durability-browser-performance.mjs", 140],
  ["src-tauri/src/app/cooperative_exit.rs", 320],
  ["src-tauri/src/app/database_migrations/mod.rs", 120],
  ["src-tauri/src/app/database_migrations/newspaper_clipping_drafts.rs", 300],
  ["src-tauri/src/app/newspaper_clipping_note_durability_baseline.rs", 360],
  ["src-tauri/src/providers/newspaper/clipping_draft_models.rs", 180],
  ["src-tauri/src/providers/newspaper/clipping_draft_repository.rs", 340],
  ["src-tauri/src/providers/newspaper/clipping_draft_service.rs", 360],
  ["src-tauri/src/lib.rs", 359]
]);

const loaded = new Map();
for (const [relative, maximum] of budgets) {
  const text = await source(relative);
  loaded.set(relative, text);
  assert.ok(lines(text) <= maximum, `${relative} exceeds its ${maximum}-line durability budget`);
}

const app = loaded.get("src/App.tsx");
const lib = loaded.get("src-tauri/src/lib.rs");
const exitBridge = loaded.get("src/components/newspaper/useClippingNoteExitBridge.ts");
const durabilityApi = loaded.get("src/components/newspaper/clipping-note-durability-api.ts");
const newspaperApi = await source("src/components/newspaper/newspaper-api.ts");
const packageJson = JSON.parse(await source("package.json"));
const performanceReport = JSON.parse(await source("../../docs/performance/newspaper-clipping-note-durability-windows-2026-08-10.json"));

assert.ok(app.includes("useClippingNoteExitBridge") && !app.includes("linkvault://prepare-exit"), "App owns native exit protocol instead of the exit bridge");
assert.ok(exitBridge.includes("linkvault://prepare-exit") && exitBridge.includes("resolve_cooperative_exit"), "renderer exit bridge omits tokenized native preparation");
assert.ok(lib.includes("WindowEvent::CloseRequested") && lib.includes("api.prevent_close()"), "native close-X is not synchronously prevented");
assert.ok(lib.includes("consume_exit_authorization") && lib.includes("api.prevent_exit()"), "unconfirmed native exits are not fail-closed");
assert.ok(lib.includes("window.hide()") && lib.includes("request_cooperative_exit(app, ExitReason::Exit)"), "close-to-tray or tray Quit bypasses the native coordinator");
const exitRequested = lib.slice(lib.indexOf("RunEvent::ExitRequested"), lib.indexOf("RunEvent::Exit =>"));
assert.ok(!exitRequested.includes("shutdown_crop_service") && !exitRequested.includes("DatabaseWriter"), "database shutdown occurs before durability authorization");

for (const forbidden of ["localStorage", "sessionStorage", "beforeunload", "pagehide"]) {
  for (const [relative, text] of loaded) {
    if (!relative.includes("clipping-note") && !relative.includes("useClippingNote")) continue;
    assert.ok(!text.includes(forbidden), `${relative} uses ${forbidden} as durability authority`);
  }
}
for (const command of [
  "checkpoint_newspaper_clipping_note",
  "load_newspaper_clipping_note_recovery",
  "claim_newspaper_clipping_note_recovery",
  "discard_newspaper_clipping_note_recovery"
]) {
  assert.ok(durabilityApi.includes(command), `durability API omits ${command}`);
  assert.ok(!newspaperApi.includes(command), `recovery command ${command} leaked into newspaper-api.ts`);
}

async function filesUnder(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await filesUnder(full));
    else files.push(full);
  }
  return files;
}
for (const file of await filesUnder(path.join(desktop, "src-tauri/src"))) {
  if (!file.endsWith(".rs")) continue;
  const normalized = file.replaceAll("\\", "/");
  const text = await readFile(file, "utf8");
  if (!text.includes("newspaper_clipping_note_drafts")) continue;
  assert.ok(
    normalized.endsWith("database_migrations/newspaper_clipping_drafts.rs")
      || normalized.endsWith("newspaper/clipping_draft_repository.rs")
      || normalized.endsWith("app/newspaper_clipping_note_durability_baseline.rs")
      || normalized.endsWith("clipping_service/tests/tests.rs"),
    `recovery draft SQL escaped its owned migration/repository seams: ${normalized}`
  );
}

for (const required of [
  "verify:clipping-note-autosave",
  "verify:clipping-note-lifecycle",
  "verify:clipping-note-durability-structure",
  "verify:clipping-note-durability-browser"
]) assert.ok(packageJson.scripts[required], `package script ${required} is missing`);

const cargoManifest = await source("src-tauri/Cargo.toml");
const durabilityBaseline = loaded.get("src-tauri/src/app/newspaper_clipping_note_durability_baseline.rs");
assert.ok(cargoManifest.includes("durability-baseline = []"), "release durability baseline feature is missing");
assert.ok(cargoManifest.includes('required-features = ["durability-baseline"]'), "durability example is not feature-gated");
assert.ok(durabilityBaseline.includes("TEN_MINUTE_MAX_WAIT_WRITES: usize = 300"), "ten-minute write bound is not measured");
assert.ok(!durabilityBaseline.includes("thread::sleep"), "durability collector uses time-based sleeps");
assert.equal(performanceReport.source.nativeCollectorCommit, "879dab1", "durability report lost clean native provenance");
assert.equal(performanceReport.source.browserMatrixCommit, "17599b8", "durability report lost clean browser provenance");
assert.equal(performanceReport.tenMinuteEquivalent.checkpointWrites, 300, "durability report does not contain the approved ten-minute workload");
assert.equal(performanceReport.tenMinuteEquivalent.failedWrites, 0, "durability report records failed checkpoint writes");
assert.equal(performanceReport.tenMinuteEquivalent.finalDraftRows, 1, "durability report no longer proves one-row upsert behavior");
assert.equal(performanceReport.nativeUat.status, "passed_observed_close_and_tray_scenarios", "native close/tray UAT status is missing");
assert.ok(performanceReport.nativeUat.notClaimed.includes("forced-process crash recovery outside a disposable test profile"), "native UAT overclaims crash recovery");

console.log("Clipping note durability ownership, size, native authority, SQL, and gate contracts passed.");
