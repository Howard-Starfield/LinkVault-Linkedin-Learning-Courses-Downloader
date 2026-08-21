import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const desktopDirectory = path.resolve(scriptDirectory, "..");
const sourceDirectory = path.join(desktopDirectory, "src-tauri", "src");

const expectedRootRustFiles = new Set(["lib.rs", "main.rs"]);
const expectedOwnedDirectories = ["app", "providers", "workflow"];
const requiredProviderDirectories = ["coursera", "linkedin", "newspaper"];

function fail(message) {
  throw new Error(`Architecture verification failed: ${message}`);
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function rustCodeOnly(source) {
  return source
    .replace(/r(#{0,16})"[\s\S]*?"\1/g, " ")
    .replace(/"(?:\\.|[^"\\])*"/gs, " ")
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/\/\/.*$/gm, " ");
}

function importsProvider(source, provider) {
  const normalizedSource = rustCodeOnly(source)
    .replace(/\s*::\s*/g, "::")
    .replace(/\s+/g, " ");
  const escapedProvider = escapeRegExp(provider);
  const providerNamespaceAliases = ["providers"];
  const crateNamespaceAliases = ["crate"];
  for (const match of normalizedSource.matchAll(
    /\buse\s+crate\s+as\s+([A-Za-z_][A-Za-z0-9_]*)\s*;/g,
  )) {
    crateNamespaceAliases.push(match[1]);
  }
  for (const match of normalizedSource.matchAll(
    /\buse\s+crate::\{[^;]*\bself\s+as\s+([A-Za-z_][A-Za-z0-9_]*)[^;]*;/g,
  )) {
    crateNamespaceAliases.push(match[1]);
  }
  for (const crateAlias of crateNamespaceAliases) {
    providerNamespaceAliases.push(`${crateAlias}::providers`);
  }
  for (const match of normalizedSource.matchAll(
    /\buse\s+(?:crate::|(?:super::)+)providers\s+as\s+([A-Za-z_][A-Za-z0-9_]*)\s*;/g,
  )) {
    providerNamespaceAliases.push(match[1]);
  }
  for (const match of normalizedSource.matchAll(
    /\buse\s+crate::\{\s*providers\s+as\s+([A-Za-z_][A-Za-z0-9_]*)[^;]*;/g,
  )) {
    providerNamespaceAliases.push(match[1]);
  }
  for (const match of normalizedSource.matchAll(
    /\buse\s+(?:crate::providers|crate::\{\s*providers)::\{[^;]*\bself\s+as\s+([A-Za-z_][A-Za-z0-9_]*)[^;]*;/g,
  )) {
    providerNamespaceAliases.push(match[1]);
  }

  const directPaths = [
    new RegExp(`\\bcrate::${escapedProvider}(?:::|\\b)`),
    new RegExp(`\\b(?:super::)+${escapedProvider}(?:::|\\b)`),
    ...providerNamespaceAliases.map(
      (namespace) =>
        new RegExp(
          `\\b${escapeRegExp(namespace)}::${escapedProvider}(?:::|\\b)`,
        ),
    ),
  ];
  if (directPaths.some((pattern) => pattern.test(normalizedSource))) {
    return true;
  }

  const groupedProviderImports = providerNamespaceAliases.map(
    (namespace) =>
      new RegExp(
        `\\b${escapeRegExp(namespace)}::\\{[^;]{0,2048}\\b${escapedProvider}\\b`,
      ),
  );
  const groupedRootImport = new RegExp(
    `\\bcrate::\\{[^;]{0,2048}\\b${escapedProvider}\\b`,
  );
  const groupedRelativeImport = new RegExp(
    `\\b(?:super::)+\\{[^;]{0,2048}\\b${escapedProvider}\\b`,
  );
  return [
    ...groupedProviderImports,
    groupedRootImport,
    groupedRelativeImport,
  ].some((pattern) => pattern.test(normalizedSource));
}

function referencesCrateExport(source, exportedName) {
  const normalizedSource = rustCodeOnly(source)
    .replace(/\s*::\s*/g, "::")
    .replace(/\s+/g, " ");
  const escapedExport = escapeRegExp(exportedName);
  const crateAliases = ["crate"];
  for (const match of normalizedSource.matchAll(
    /\buse\s+crate\s+as\s+([A-Za-z_][A-Za-z0-9_]*)\s*;/g,
  )) {
    crateAliases.push(match[1]);
  }
  for (const match of normalizedSource.matchAll(
    /\buse\s+crate::\{[^;]*\bself\s+as\s+([A-Za-z_][A-Za-z0-9_]*)[^;]*;/g,
  )) {
    crateAliases.push(match[1]);
  }
  return crateAliases.some((crateAlias) => {
    const escapedAlias = escapeRegExp(crateAlias);
    return [
      new RegExp(`\\b${escapedAlias}::${escapedExport}(?:::|\\b)`),
      new RegExp(
        `\\b${escapedAlias}::\\{[^;]{0,2048}\\b${escapedExport}\\b`,
      ),
    ].some((pattern) => pattern.test(normalizedSource));
  });
}

function usesForbiddenNamespaceAlias(source) {
  const normalizedSource = rustCodeOnly(source)
    .replace(/\s*::\s*/g, "::")
    .replace(/\s+/g, " ");
  const forbiddenAliases = [
    /\buse\s+crate(?:::providers)?\s+as\s+[A-Za-z_][A-Za-z0-9_]*\s*;/,
    /\buse\s+crate::\{[^;]*\b(?:self|providers)\s+as\s+[A-Za-z_][A-Za-z0-9_]*/,
    /\buse\s+(?:super::)*super\s+as\s+[A-Za-z_][A-Za-z0-9_]*\s*;/,
    /\buse\s+[A-Za-z_][A-Za-z0-9_]*\s+as\s+[A-Za-z_][A-Za-z0-9_]*\s*;/,
    /\buse\s+[A-Za-z_][A-Za-z0-9_]*::providers\s+as\s+[A-Za-z_][A-Za-z0-9_]*\s*;/,
    /\buse\s+[A-Za-z_][A-Za-z0-9_]*::\{[^;]*\bproviders\s+as\s+[A-Za-z_][A-Za-z0-9_]*/,
    /\buse\s+[A-Za-z_][A-Za-z0-9_]*::providers::\{[^;]*\bself\s+as\s+[A-Za-z_][A-Za-z0-9_]*/,
  ];
  return forbiddenAliases.some((pattern) => pattern.test(normalizedSource));
}

const providerImportNegativeFixtures = [
  "use crate::providers::coursera::client;",
  "use crate :: providers :: { coursera as course };",
  "pub use crate::{providers::{coursera as course}};",
  "pub use crate::{coursera as course};",
  "use super::super::coursera::models;",
  "use super::{coursera as course};",
  "pub use crate::providers::coursera::*;",
  "use crate::providers as p; use p::coursera::client;",
  "use crate::providers::{self as p}; use p::coursera::client;",
  "use crate as root; use root::providers::coursera::client;",
  "use crate::{self as root}; use root::providers::{coursera};",
  "let value = crate::coursera::value();",
];
for (const fixture of providerImportNegativeFixtures) {
  if (!importsProvider(fixture, "coursera")) {
    fail(`provider import negative fixture escaped detection: ${fixture}`);
  }
}
if (importsProvider("use crate::providers::youtube::models;", "coursera")) {
  fail("provider import detector rejected an unrelated provider fixture");
}
if (
  !referencesCrateExport(
    "use crate as root; use root::{artifact_downloader as downloader};",
    "artifact_downloader",
  )
) {
  fail("crate-root compatibility re-export fixture escaped detection");
}
for (const fixture of [
  "use crate::providers as p1; use p1 as p2; use p2::coursera::client;",
  "use crate as root; use root::providers as p; use p::coursera::client;",
]) {
  if (!usesForbiddenNamespaceAlias(fixture)) {
    fail(`namespace alias negative fixture escaped detection: ${fixture}`);
  }
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
for (const provider of requiredProviderDirectories) {
  await requireDirectory(providersDirectory, provider);
}

const providerDirectories = (await readdir(providersDirectory, { withFileTypes: true }))
  .filter((entry) => entry.isDirectory())
  .map((entry) => entry.name)
  .sort();

const providersModuleSource = await readFile(
  path.join(providersDirectory, "mod.rs"),
  "utf8",
);
const declaredProviderModules = [
  ...providersModuleSource.matchAll(/\bpub\s+mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;/g),
]
  .map((match) => match[1])
  .sort();
if (
  declaredProviderModules.length !== providerDirectories.length ||
  declaredProviderModules.some(
    (provider, index) => provider !== providerDirectories[index],
  )
) {
  fail(
    `providers/mod.rs declarations (${declaredProviderModules.join(", ")}) do not match provider directories (${providerDirectories.join(", ")})`,
  );
}

const requiredModuleFiles = [
  path.join(sourceDirectory, "app", "mod.rs"),
  path.join(sourceDirectory, "providers", "mod.rs"),
  ...providerDirectories.map((provider) =>
    path.join(sourceDirectory, "providers", provider, "mod.rs")
  ),
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

if (/\buse\b[^;]*\bproviders\b[^;]*\bas\b/s.test(libSource)) {
  fail("lib.rs aliases a provider re-export and can bypass dependency checks");
}

const normalizedLibSource = rustCodeOnly(libSource)
  .replace(/\s*::\s*/g, "::")
  .replace(/\s+/g, " ");
const crateRootProviderExports = new Map();
for (const match of normalizedLibSource.matchAll(
  /\bpub\s+use\s+providers::([A-Za-z_][A-Za-z0-9_]*)::\{([^}]*)\}\s*;/g,
)) {
  const provider = match[1];
  for (const member of match[2].split(",")) {
    const exportedName = member.trim().split(/\s+as\s+/).at(-1);
    if (exportedName && exportedName !== "self") {
      crateRootProviderExports.set(exportedName, provider);
    }
  }
}
for (const match of normalizedLibSource.matchAll(
  /\bpub\s+use\s+providers::\{([^}]*)\}\s*;/g,
)) {
  for (const member of match[1].split(",")) {
    const provider = member.trim().split(/\s+as\s+/)[0];
    const exportedName = member.trim().split(/\s+as\s+/).at(-1);
    if (provider && exportedName) {
      crateRootProviderExports.set(exportedName, provider);
    }
  }
}
for (const match of normalizedLibSource.matchAll(
  /\bpub\s+use\s+providers::([A-Za-z_][A-Za-z0-9_]*)\s*;/g,
)) {
  crateRootProviderExports.set(match[1], match[1]);
}

if (/\b(?:pub\s+)?use\b/.test(providersModuleSource)) {
  fail("providers/mod.rs re-exports or aliases provider internals");
}

const unsafeModuleIndirection =
  /#\s*\[\s*path\s*=\s*"[^"]*(?:\.\.|providers[\\/])|\binclude!\s*\(/s;

for (const provider of providerDirectories) {
  const providerDirectory = path.join(providersDirectory, provider);
  const otherProviders = providerDirectories.filter(
    (candidate) => candidate !== provider,
  );

  for (const rustFile of await collectRustFiles(providerDirectory)) {
    const source = await readFile(rustFile, "utf8");
    if (usesForbiddenNamespaceAlias(source)) {
      fail(
        `${path.relative(desktopDirectory, rustFile)} aliases a crate/provider namespace and can bypass dependency checks`,
      );
    }
    if (unsafeModuleIndirection.test(source)) {
      fail(
        `${path.relative(desktopDirectory, rustFile)} uses provider-crossing path/include indirection`,
      );
    }
    for (const otherProvider of otherProviders) {
      if (importsProvider(source, otherProvider)) {
        fail(
          `${path.relative(desktopDirectory, rustFile)} imports provider ${otherProvider} directly`,
        );
      }
    }
    for (const [exportedName, owningProvider] of crateRootProviderExports) {
      if (
        owningProvider !== provider &&
        referencesCrateExport(source, exportedName)
      ) {
        fail(
          `${path.relative(desktopDirectory, rustFile)} imports crate-root provider compatibility export ${exportedName} from ${owningProvider}`,
        );
      }
    }

    if (provider === "youtube") {
      const relativeProviderFile = path
        .relative(providerDirectory, rustFile)
        .replaceAll("\\", "/");
      const forbiddenYouTubeOwnershipFile =
        /(?:^|\/)(?:scheduler|runtime|cancellation|job_store|event_store|retry_queue)\.rs$/;
      const forbiddenYouTubeOwnership =
        /\b(?:struct|enum|type)\s+\w*(?:Scheduler|Runtime|CancellationRegistry|JobTable|EventTable|RetryQueue)\b|\bCREATE\s+TABLE\b/i;
      if (
        forbiddenYouTubeOwnershipFile.test(relativeProviderFile) ||
        forbiddenYouTubeOwnership.test(source)
      ) {
        fail(
          `${path.relative(desktopDirectory, rustFile)} declares workflow lifecycle ownership inside YouTube`,
        );
      }

      const forbiddenYouTubeProcessLaunch =
        /(?:\buse\s+std::process\b|\bstd::process::Command\b|\buse\s+tokio::process\b|\btokio::process::Command\b|\bCommand\s*::\s*new\b|\bCreateProcessW\b)/;
      if (forbiddenYouTubeProcessLaunch.test(source)) {
        fail(
          `${path.relative(desktopDirectory, rustFile)} launches a process inside YouTube; providers may only submit typed helper requests to workflow::transient`,
        );
      }
    }
  }
}

const workflowDirectory = path.join(sourceDirectory, "workflow");
for (const rustFile of await collectRustFiles(workflowDirectory)) {
  const source = await readFile(rustFile, "utf8");
  if (usesForbiddenNamespaceAlias(source)) {
    fail(
      `${path.relative(desktopDirectory, rustFile)} aliases a crate/provider namespace and can bypass dependency checks`,
    );
  }
  if (unsafeModuleIndirection.test(source)) {
    fail(
      `${path.relative(desktopDirectory, rustFile)} uses provider path/include indirection`,
    );
  }
  for (const provider of providerDirectories) {
    if (importsProvider(source, provider)) {
      fail(
        `${path.relative(desktopDirectory, rustFile)} imports provider ${provider}; workflow must remain provider-agnostic`,
      );
    }
  }
  for (const [exportedName, owningProvider] of crateRootProviderExports) {
    if (referencesCrateExport(source, exportedName)) {
      fail(
        `${path.relative(desktopDirectory, rustFile)} imports crate-root provider compatibility export ${exportedName} from ${owningProvider}; workflow must remain provider-agnostic`,
      );
    }
  }
}

const allowedLegacyAppProviderImports = new Map([
  ["app/database.rs", new Set(["newspaper"])],
  ["app/newspaper_clipping_crop_baseline.rs", new Set(["newspaper"])],
  ["app/newspaper_clipping_note_durability_baseline.rs", new Set(["newspaper"])],
]);
const appDirectory = path.join(sourceDirectory, "app");
for (const rustFile of await collectRustFiles(appDirectory)) {
  const source = await readFile(rustFile, "utf8");
  if (usesForbiddenNamespaceAlias(source)) {
    fail(
      `${path.relative(desktopDirectory, rustFile)} aliases a crate/provider namespace and can bypass dependency checks`,
    );
  }
  const relativeFile = path.relative(sourceDirectory, rustFile).replaceAll("\\", "/");
  const allowedProviders = allowedLegacyAppProviderImports.get(relativeFile) ?? new Set();
  for (const provider of providerDirectories) {
    const importsCurrentProvider = importsProvider(source, provider);
    const reexportsCurrentProvider = new RegExp(
      `\\bpub(?:\\([^)]*\\))?\\s+use\\b[^;]{0,2048}(?:providers::)?${escapeRegExp(provider)}\\b`,
      "s",
    ).test(source.replace(/\s*::\s*/g, "::"));
    if (
      importsCurrentProvider &&
      (!allowedProviders.has(provider) || reexportsCurrentProvider)
    ) {
      fail(
        `${path.relative(desktopDirectory, rustFile)} imports provider ${provider}; app-to-provider exceptions cannot expand`,
      );
    }
  }
  for (const [exportedName, owningProvider] of crateRootProviderExports) {
    if (
      referencesCrateExport(source, exportedName) &&
      !allowedProviders.has(owningProvider)
    ) {
      fail(
        `${path.relative(desktopDirectory, rustFile)} imports crate-root provider compatibility export ${exportedName} from ${owningProvider}; app exceptions cannot expand`,
      );
    }
  }
}

console.log(
  "Architecture verification passed: app, workflow, and all provider sources have explicit owners.",
);
