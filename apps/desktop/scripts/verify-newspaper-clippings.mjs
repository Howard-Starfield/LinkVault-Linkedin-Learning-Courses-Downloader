import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const desktopDirectory = path.resolve(scriptDirectory, "..");
const repositoryDirectory = path.resolve(desktopDirectory, "..", "..");
const rustDirectory = path.join(desktopDirectory, "src-tauri", "src");
const newspaperDirectory = path.join(rustDirectory, "providers", "newspaper");
const performanceDirectory = path.join(repositoryDirectory, "docs", "performance");

function fail(message) {
  throw new Error(`Newspaper clipping verification failed: ${message}`);
}

function requireContains(source, fragment, file) {
  if (!source.includes(fragment)) {
    fail(`${file} is missing required contract fragment ${JSON.stringify(fragment)}`);
  }
}

function requireOrder(source, fragments, file) {
  let previous = -1;
  for (const fragment of fragments) {
    const next = source.indexOf(fragment, previous + 1);
    if (next === -1) {
      fail(`${file} is missing required ordered fragment ${JSON.stringify(fragment)}`);
    }
    if (next < previous) {
      fail(`${file} moved ${JSON.stringify(fragment)} before an earlier lifecycle step`);
    }
    previous = next;
  }
}

function productionSource(source) {
  const testBoundary = source.indexOf("#[cfg(test)]");
  return testBoundary === -1 ? source : source.slice(0, testBoundary);
}

function requireKeys(value, keys, description) {
  for (const key of keys) {
    if (!(key in value)) {
      fail(`${description} is missing required field ${key}`);
    }
  }
}

const [
  cropSource,
  serviceSource,
  commandSource,
  modelSource,
  moduleSource,
  libSource,
  apiSource,
  readerSource,
  librarySource,
  cargoSource,
  baselineHarnessSource,
] = await Promise.all([
  readFile(path.join(newspaperDirectory, "clipping_crop.rs"), "utf8"),
  readFile(path.join(newspaperDirectory, "clipping_service.rs"), "utf8"),
  readFile(path.join(newspaperDirectory, "commands.rs"), "utf8"),
  readFile(path.join(newspaperDirectory, "clipping_models.rs"), "utf8"),
  readFile(path.join(newspaperDirectory, "mod.rs"), "utf8"),
  readFile(path.join(rustDirectory, "lib.rs"), "utf8"),
  readFile(path.join(desktopDirectory, "src", "components", "newspaper", "newspaper-api.ts"), "utf8"),
  readFile(path.join(desktopDirectory, "src", "components", "newspaper", "NewspaperReader.tsx"), "utf8"),
  readFile(path.join(desktopDirectory, "src", "components", "newspaper", "NewspaperLibrary.tsx"), "utf8"),
  readFile(path.join(desktopDirectory, "src-tauri", "Cargo.toml"), "utf8"),
  readFile(path.join(rustDirectory, "app", "newspaper_clipping_crop_baseline.rs"), "utf8"),
]);

const productionCrop = productionSource(cropSource);
const productionService = productionSource(serviceSource);
const productionCommands = productionSource(commandSource);

for (const [source, file] of [
  [cropSource, "newspaper/clipping_crop.rs"],
  [serviceSource, "newspaper/clipping_service.rs"],
]) {
  if (source.includes("#[ignore")) {
    fail(`${file} must not leave a skipped crop test in the Phase 2 suite`);
  }
}

requireContains(moduleSource, "pub mod clipping_crop;", "newspaper/mod.rs");
requireContains(moduleSource, "pub mod clipping_service;", "newspaper/mod.rs");
requireContains(
  libSource,
  "pub use app::newspaper_clipping_crop_baseline as crop_baseline;",
  "lib.rs",
);
requireContains(libSource, "newspaper::commands::create_newspaper_clipping", "lib.rs");
for (const fragment of [
  "crop-baseline = []",
  "newspaper_clipping_crop_baseline",
  "required-features = [\"crop-baseline\"]",
]) {
  requireContains(cargoSource, fragment, "src-tauri/Cargo.toml");
}
for (const fragment of ["MAX_SOURCE_PIXELS", "ClippingService::new", "register_staged", "release measurement harness"]) {
  requireContains(baselineHarnessSource, fragment, "app/newspaper_clipping_crop_baseline.rs");
}
if (baselineHarnessSource.includes("#[ignore")) {
  fail("app/newspaper_clipping_crop_baseline.rs must remain a repeatable command, not a skipped test");
}
requireOrder(
  productionCommands,
  [
    "pub async fn create_newspaper_clipping",
    "tauri::async_runtime::spawn_blocking",
    "service.create_newspaper_clipping",
    "CreateNewspaperClippingFailure::from_code",
  ],
  "newspaper/commands.rs",
);

for (const fragment of [
  "pub struct NormalizedCropRect",
  "pub struct CreateNewspaperClippingRequest",
  "pub struct CreateNewspaperClippingResponse",
  "pub struct CreateNewspaperClippingFailure",
  "InvalidCropRect",
  "SourceMediaPathInvalid",
  "SourceMediaChangedDuringRead",
  "OutputValidationFailed",
  "ServiceUnavailable",
]) {
  requireContains(modelSource, fragment, "newspaper/clipping_models.rs");
}

for (const fragment of [
  "NORMALIZED_EPSILON",
  "MAX_SOURCE_FILE_BYTES",
  "MAX_SOURCE_DIMENSION",
  "MAX_SOURCE_PIXELS",
  "MAX_OUTPUT_BYTES",
  "MIN_CROP_WIDTH",
  "MIN_CROP_HEIGHT",
  "validate_create_request",
  "validate_normalized_rect",
  "to_source_pixels",
  "resolve_best_source",
  "read_candidate_with_after_read",
  "symlink_metadata",
  "is_symlink_or_reparse",
  "canonicalize",
  "starts_with(root)",
  "apply_jpeg_exif_orientation",
  "encode_lossless_webp",
  "validate_lossless_output",
  "layout.write_staging",
  "validate_source_recheck",
]) {
  requireContains(productionCrop, fragment, "newspaper/clipping_crop.rs");
}
for (const forbidden of ["DatabaseWriter", "Connection::open(", ".execute("]) {
  if (productionCrop.includes(forbidden)) {
    fail(`newspaper/clipping_crop.rs must not perform database write work (${forbidden})`);
  }
}

const createBoundary = productionService.slice(
  productionService.indexOf("fn create_newspaper_clipping_inner"),
  productionService.indexOf("fn response_from_clipping"),
);
requireOrder(
  createBoundary,
  [
    "validate_create_request(&request)",
    "let _permit = self",
    "let source_record = self",
    "clipping_crop::stage_crop",
    "clipping_crop::validate_source_recheck",
    "self.layout.discard_staging(&request.operation_id)",
    "self.register_staged(record)",
  ],
  "newspaper/clipping_service.rs",
);
for (const fragment of [
  "crop_accepting",
  "shutdown_crop_service",
  "ClippingAssetState::DeletePending",
  "ClippingAssetState::Creating",
  "recover_creating_id",
]) {
  requireContains(productionService, fragment, "newspaper/clipping_service.rs");
}

const apiBoundary = apiSource.slice(
  apiSource.indexOf("export type NormalizedCropRect"),
  apiSource.indexOf("export type EnsureThumbnailResult"),
);
for (const fragment of [
  "operationId: string",
  "pageId: string",
  "expectedMediaVersion: number",
  "assetByteCount: number",
  "safeMessage: string",
]) {
  requireContains(apiBoundary, fragment, "newspaper-api.ts clipping contract");
}
requireContains(apiSource, "createNewspaperClipping", "newspaper-api.ts command adapter");
requireContains(apiSource, '"create_newspaper_clipping"', "newspaper-api.ts command adapter");
for (const forbidden of ["sourcePath", "assetPath", "relativePath", "outputDir", "tone", "zoom"]) {
  if (apiBoundary.includes(forbidden)) {
    fail(`newspaper-api.ts clipping contract leaks a filesystem path field (${forbidden})`);
  }
}
for (const [source, file] of [
  [readerSource, "NewspaperReader.tsx"],
  [librarySource, "NewspaperLibrary.tsx"],
]) {
  if (source.includes("createNewspaperClipping")) {
    fail(`${file} must remain untouched by the backend-only Phase 2 command`);
  }
}

const performanceEntries = (await readdir(performanceDirectory, { withFileTypes: true }))
  .filter((entry) => entry.isFile() && /^newspaper-clippings-crop-windows-\d{4}-\d{2}-\d{2}\.json$/.test(entry.name))
  .map((entry) => entry.name)
  .sort();
if (performanceEntries.length === 0) {
  fail("missing docs/performance/newspaper-clippings-crop-windows-YYYY-MM-DD.json release baseline");
}
const baselinePath = path.join(performanceDirectory, performanceEntries.at(-1));
const baseline = JSON.parse(await readFile(baselinePath, "utf8"));
requireKeys(
  baseline,
  [
    "schemaVersion",
    "commit",
    "build",
    "measurementDate",
    "command",
    "commandElapsedMs",
    "sampleMethod",
    "persistenceTimingScope",
    "machine",
    "toolchain",
    "cases",
    "maxConcurrentCropSections",
    "sqliteBusyFailures",
    "concurrencyEvidence",
    "uiStallEvidence",
  ],
  "crop baseline",
);
assert.equal(baseline.schemaVersion, 1, "crop baseline schemaVersion changed");
assert.match(baseline.commit, /^[0-9a-f]{40}$/, "crop baseline must identify a full commit SHA");
assert.equal(baseline.build, "release", "crop baseline must use a release build");
assert.match(baseline.measurementDate, /^\d{4}-\d{2}-\d{2}$/, "crop baseline measurementDate is invalid");
assert.ok(baseline.command.includes("--release"), "crop baseline command must use a release build");
assert.ok(Number.isFinite(baseline.commandElapsedMs) && baseline.commandElapsedMs > 0);
assert.ok(typeof baseline.sampleMethod === "string" && baseline.sampleMethod.length > 0);
assert.ok(typeof baseline.persistenceTimingScope === "string" && baseline.persistenceTimingScope.length > 0);
if ("sourceTree" in baseline) {
  assert.ok(
    typeof baseline.sourceTree === "string" && baseline.sourceTree.length > 0,
    "crop baseline sourceTree provenance must be a non-empty string",
  );
}
requireKeys(
  baseline.machine,
  ["os", "cpu", "logicalCores", "ramBytes"],
  "crop baseline machine",
);
assert.equal(baseline.machine.os, "Windows", "crop baseline must identify its Windows host");
assert.ok(Number.isInteger(baseline.machine.logicalCores) && baseline.machine.logicalCores > 0);
assert.ok(Number.isInteger(baseline.machine.ramBytes) && baseline.machine.ramBytes > 0);
requireKeys(baseline.toolchain, ["rustc", "host", "llvm", "visualStudioEnvironment"], "crop baseline toolchain");
for (const key of ["rustc", "host", "llvm", "visualStudioEnvironment"]) {
  assert.ok(typeof baseline.toolchain[key] === "string" && baseline.toolchain[key].length > 0);
}
assert.equal(baseline.maxConcurrentCropSections, 1, "V1 crop concurrency must remain one");
assert.equal(baseline.sqliteBusyFailures, 0, "crop baseline recorded SQLITE_BUSY failures");
assert.ok(typeof baseline.concurrencyEvidence === "string" && baseline.concurrencyEvidence.length > 0);
assert.ok(typeof baseline.uiStallEvidence === "string" && baseline.uiStallEvidence.length > 0);

const requiredCaseKeys = [
  "caseId",
  "sourceFormat",
  "selectedSourceKind",
  "sourceWidth",
  "sourceHeight",
  "sourceBytes",
  "cropWidth",
  "cropHeight",
  "outputBytes",
  "queueWaitMs",
  "readMs",
  "decodeMs",
  "cropMs",
  "encodeMs",
  "validateMs",
  "filesystemMs",
  "databaseMs",
  "totalMs",
  "workingSetDeltaBytes",
];
assert.ok(Array.isArray(baseline.cases) && baseline.cases.length >= 3, "crop baseline needs three generated cases");
const sourceFormats = new Set();
for (const [index, cropCase] of baseline.cases.entries()) {
  for (const key of requiredCaseKeys) {
    if (!(key in cropCase)) {
      fail(`crop baseline case ${index} is missing ${key}`);
    }
  }
  sourceFormats.add(cropCase.sourceFormat);
  for (const key of requiredCaseKeys.slice(3)) {
    assert.ok(Number.isFinite(cropCase[key]), `crop baseline case ${index} has non-finite ${key}`);
  }
  assert.ok(typeof cropCase.caseId === "string" && cropCase.caseId.length > 0);
  assert.equal(cropCase.selectedSourceKind, "original", `crop baseline case ${index} source priority changed`);
  for (const key of ["sourceWidth", "sourceHeight", "sourceBytes", "cropWidth", "cropHeight", "outputBytes"]) {
    assert.ok(cropCase[key] > 0, `crop baseline case ${index} has invalid ${key}`);
  }
}
assert.deepEqual(sourceFormats, new Set(["jpeg", "png", "webp"]), "crop baseline source-format coverage changed");

console.log(
  `Newspaper clipping structural and release-baseline contracts passed (${path.relative(repositoryDirectory, baselinePath).replaceAll("\\", "/")}).`,
);
