import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const desktopDirectory = path.resolve(scriptDirectory, "..");
const rustSourceDirectory = path.join(desktopDirectory, "src-tauri", "src");
const legacyWriteBaseline = JSON.parse(
  await readFile(
    path.join(scriptDirectory, "persistence-legacy-write-baseline.json"),
    "utf8",
  ),
);

function fail(message) {
  throw new Error(`Persistence verification failed: ${message}`);
}

async function collectRustFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];

  for (const entry of entries) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await collectRustFiles(entryPath)));
    } else if (entry.isFile() && entry.name.endsWith(".rs")) {
      files.push(entryPath);
    }
  }

  return files;
}

function productionSource(source) {
  const testBoundary = source.indexOf("#[cfg(test)]");
  return testBoundary === -1 ? source : source.slice(0, testBoundary);
}

const providersDirectory = path.join(rustSourceDirectory, "providers");
const directConnectionAllowlist = new Set([
  path.join("providers", "linkedin", "browser_cookies.rs"),
]);
const directSqlPrimitives = [
  ".execute(",
  ".execute_batch(",
  ".transaction(",
  ".unchecked_transaction(",
  ".transaction_with_behavior(",
];
const sharedWriteHelpers = new Set(
  Object.values(legacyWriteBaseline.sharedWriteHelperCallCounts).flatMap(
    (counts) => Object.keys(counts),
  ),
);
const observedProviderSources = new Map();

for (const rustFile of await collectRustFiles(providersDirectory)) {
  if (path.basename(rustFile) === "tests.rs") {
    continue;
  }
  const source = productionSource(await readFile(rustFile, "utf8"));
  const relativeSourcePath = path.relative(rustSourceDirectory, rustFile);
  const baselinePath = path
    .relative(desktopDirectory, rustFile)
    .replaceAll("\\", "/");
  observedProviderSources.set(baselinePath, source);
  if (
    source.includes("Connection::open(") &&
    !directConnectionAllowlist.has(relativeSourcePath)
  ) {
    fail(
      `${path.relative(desktopDirectory, rustFile)} opens a database directly outside the explicit external-database allowlist`,
    );
  }
  if (source.includes("initialize_database(")) {
    fail(
      `${path.relative(desktopDirectory, rustFile)} still initializes schemas during runtime work`,
    );
  }
  if (
    source.includes("storage::initialize(") ||
    source.includes("cache::initialize(")
  ) {
    fail(
      `${path.relative(desktopDirectory, rustFile)} still invokes a provider schema or seed initializer during runtime work`,
    );
  }
}

for (const [baselinePath, source] of observedProviderSources) {
  if (baselinePath.endsWith("/linkedin/browser_cookies.rs")) {
    continue;
  }
  const observedDirectSql = directSqlPrimitives.reduce(
    (total, primitive) => total + source.split(primitive).length - 1,
    0,
  );
  const expectedDirectSql =
    legacyWriteBaseline.directSqlPrimitiveCounts[baselinePath] ?? 0;
  if (observedDirectSql !== expectedDirectSql) {
    fail(
      `${baselinePath} has ${observedDirectSql} direct SQL write primitives; reviewed legacy baseline is ${expectedDirectSql}`,
    );
  }

  const expectedHelpers =
    legacyWriteBaseline.sharedWriteHelperCallCounts[baselinePath] ?? {};
  for (const helper of sharedWriteHelpers) {
    const observed = source.split(`${helper}(`).length - 1;
    const expected = expectedHelpers[helper] ?? 0;
    if (observed !== expected) {
      fail(
        `${baselinePath} calls legacy write helper ${helper} ${observed} times; reviewed baseline is ${expected}`,
      );
    }
  }
}

const databaseFile = path.join(rustSourceDirectory, "app", "database.rs");
const databaseSource = await readFile(databaseFile, "utf8");
for (const contract of [
  "pub const CURRENT_SCHEMA_VERSION",
  "pub fn initialize_database",
  "pub fn open_runtime",
  "persistence_gate_new_database_initializes_without_backup",
  "persistence_gate_legacy_database_is_backed_up_before_migration",
  "persistence_gate_runtime_open_does_not_modify_database",
  "persistence_gate_future_schema_is_rejected",
  "persistence_gate_connection_policy_is_consistent",
  "persistence_gate_backup_allocation_never_overwrites_existing_candidates",
  "persistence_gate_corrupt_backup_reports_safe_integrity_failure",
  "persistence_gate_failed_migration_keeps_backup_and_can_retry_idempotently",
  "persistence_gate_competing_initializer_returns_busy_without_migrating",
]) {
  if (!databaseSource.includes(contract)) {
    fail(`app/database.rs is missing contract ${contract}`);
  }
}

const writerSource = await readFile(
  path.join(rustSourceDirectory, "app", "database_writer.rs"),
  "utf8",
);
for (const contract of [
  "pub struct DatabaseWriter",
  "pub fn execute",
  "pub fn shutdown",
  "persistence_gate_writer_serializes_eight_hundred_concurrent_writes",
  "persistence_gate_reader_keeps_previous_snapshot_during_uncommitted_write",
  "persistence_gate_shutdown_drains_accepted_work_and_rejects_late_work",
  "persistence_gate_panicked_request_does_not_kill_writer",
]) {
  if (!writerSource.includes(contract)) {
    fail(`app/database_writer.rs is missing contract ${contract}`);
  }
}

const diagnosticsSource = await readFile(
  path.join(rustSourceDirectory, "app", "database_diagnostics.rs"),
  "utf8",
);
if (
  !diagnosticsSource.includes(
    "persistence_gate_diagnostics_are_bounded_and_structurally_redacted",
  )
) {
  fail("app/database_diagnostics.rs is missing its bounded redaction gate");
}

const libSource = await readFile(
  path.join(rustSourceDirectory, "lib.rs"),
  "utf8",
);
if (!libSource.includes("initialize_database_with_diagnostics(&db_path")) {
  fail("Tauri startup does not own explicit database initialization");
}
if (
  !libSource.includes("DatabaseWriter::start") ||
  !libSource.includes(".shutdown()")
) {
  fail("Tauri lifecycle does not start and drain the shared database writer");
}

console.log(
  "Persistence structure passed: startup owns migrations and providers use runtime connections.",
);
