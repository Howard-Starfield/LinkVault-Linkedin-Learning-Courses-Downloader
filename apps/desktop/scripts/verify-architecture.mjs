import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const desktopDirectory = path.resolve(scriptDirectory, "..");
const sourceDirectory = path.join(desktopDirectory, "src-tauri", "src");

const expectedRootRustFiles = new Set(["lib.rs", "main.rs"]);
const expectedOwnedDirectories = ["app", "providers", "workflow"];
const expectedProviderDirectories = ["coursera", "linkedin", "newspaper"];

function fail(message) {
  throw new Error(`Architecture verification failed: ${message}`);
}

async function requireDirectory(parent, name) {
  const entries = await readdir(parent, { withFileTypes: true });
  if (!entries.some((entry) => entry.isDirectory() && entry.name === name)) {
    fail(`missing directory ${path.relative(desktopDirectory, path.join(parent, name))}`);
  }
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

const rootEntries = await readdir(sourceDirectory, { withFileTypes: true });
const unexpectedRootRustFiles = rootEntries
  .filter(
    (entry) =>
      entry.isFile() &&
      entry.name.endsWith(".rs") &&
      !expectedRootRustFiles.has(entry.name),
  )
  .map((entry) => entry.name)
  .sort();

if (unexpectedRootRustFiles.length > 0) {
  fail(
    `provider or application Rust files remain at the crate root: ${unexpectedRootRustFiles.join(", ")}`,
  );
}

for (const directory of expectedOwnedDirectories) {
  await requireDirectory(sourceDirectory, directory);
}

const providersDirectory = path.join(sourceDirectory, "providers");
for (const provider of expectedProviderDirectories) {
  await requireDirectory(providersDirectory, provider);
}

const requiredModuleFiles = [
  path.join(sourceDirectory, "app", "mod.rs"),
  path.join(sourceDirectory, "providers", "mod.rs"),
  path.join(sourceDirectory, "providers", "coursera", "mod.rs"),
  path.join(sourceDirectory, "providers", "linkedin", "mod.rs"),
  path.join(sourceDirectory, "providers", "newspaper", "mod.rs"),
  path.join(sourceDirectory, "workflow", "mod.rs"),
];

for (const moduleFile of requiredModuleFiles) {
  const parent = path.dirname(moduleFile);
  const entries = await readdir(parent, { withFileTypes: true });
  if (!entries.some((entry) => entry.isFile() && entry.name === "mod.rs")) {
    fail(`missing module owner ${path.relative(desktopDirectory, moduleFile)}`);
  }
}

const requiredOwnedFiles = [
  path.join(sourceDirectory, "app", "database.rs"),
  path.join(sourceDirectory, "app", "security.rs"),
  path.join(sourceDirectory, "app", "storage.rs"),
  path.join(sourceDirectory, "app", "updates.rs"),
];

for (const ownedFile of requiredOwnedFiles) {
  const parent = path.dirname(ownedFile);
  const entries = await readdir(parent, { withFileTypes: true });
  const name = path.basename(ownedFile);
  if (!entries.some((entry) => entry.isFile() && entry.name === name)) {
    fail(`missing application owner ${path.relative(desktopDirectory, ownedFile)}`);
  }
}

const libSource = await readFile(path.join(sourceDirectory, "lib.rs"), "utf8");
for (const declaration of ["mod app;", "mod providers;", "pub mod workflow;"]) {
  if (!libSource.includes(declaration)) {
    fail(`lib.rs does not declare ${declaration}`);
  }
}

for (const provider of expectedProviderDirectories) {
  const providerDirectory = path.join(providersDirectory, provider);
  const otherProviders = expectedProviderDirectories.filter(
    (candidate) => candidate !== provider,
  );

  for (const rustFile of await collectRustFiles(providerDirectory)) {
    const source = await readFile(rustFile, "utf8");
    for (const otherProvider of otherProviders) {
      const forbiddenImports = [
        `crate::${otherProvider}::`,
        `crate::providers::${otherProvider}::`,
      ];
      if (forbiddenImports.some((forbidden) => source.includes(forbidden))) {
        fail(
          `${path.relative(desktopDirectory, rustFile)} imports provider ${otherProvider} directly`,
        );
      }
    }
  }
}

console.log(
  "Architecture verification passed: app, workflow, and all provider sources have explicit owners.",
);
