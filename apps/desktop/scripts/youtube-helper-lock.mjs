import { createHash, randomBytes } from "node:crypto";
import { createReadStream } from "node:fs";
import { readFile } from "node:fs/promises";
import path from "node:path";

export const REQUIRED_COMPONENTS = Object.freeze(["yt-dlp", "deno", "ffmpeg", "ffprobe"]);
export const TARGET_TRIPLE = "x86_64-pc-windows-msvc";
export const LOCK_RELATIVE_PATH = "docs/third-party/youtube-helpers-lock.json";
export const BINARY_RELATIVE_PATH = "apps/desktop/src-tauri/binaries";

export function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function fail(message) {
  throw new Error(`YouTube helper lock validation failed: ${message}`);
}

export function canonicalize(value) {
  if (Array.isArray(value)) {
    return `[${value.map((entry) => canonicalize(entry)).join(",")}]`;
  }
  if (isRecord(value)) {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalize(value[key])}`)
      .join(",")}}`;
  }
  if (typeof value === "number" && !Number.isFinite(value)) {
    fail("canonical JSON cannot contain a non-finite number");
  }
  const encoded = JSON.stringify(value);
  if (encoded === undefined) {
    fail("canonical JSON cannot contain undefined");
  }
  return encoded;
}

export function digestLock(lock) {
  if (!isRecord(lock)) fail("lock document must be an object");
  const withoutDigest = { ...lock };
  delete withoutDigest.lockDigest;
  return createHash("sha256").update(canonicalize(withoutDigest), "utf8").digest("hex");
}

export function sha256Hex(value) {
  return /^[0-9a-f]{64}$/.test(value);
}

export function isPinnedHttpsUrl(value, label) {
  if (typeof value !== "string" || value.length === 0) fail(`${label} must be a non-empty URL`);
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    fail(`${label} must be an absolute HTTPS URL`);
  }
  if (parsed.protocol !== "https:") fail(`${label} must use HTTPS`);
  if (parsed.username || parsed.password) fail(`${label} must not contain URL credentials`);
  if (/latest/i.test(parsed.pathname) || /(?:^|[._/-])latest(?:[._/-]|$)/i.test(parsed.hostname)) {
    fail(`${label} must not use a floating latest asset`);
  }
  return parsed;
}

export function safeRelativePath(value, label) {
  if (typeof value !== "string" || value.length === 0) fail(`${label} must be a non-empty relative path`);
  const normalized = value.replaceAll("\\", "/");
  if (normalized.startsWith("/") || /^[A-Za-z]:\//.test(normalized)) {
    fail(`${label} must be relative`);
  }
  const segments = normalized.split("/");
  if (segments.some((segment) => segment.length === 0 || segment === "." || segment === ".." || segment.includes(":"))) {
    fail(`${label} contains an unsafe path segment`);
  }
  return segments.join("/");
}

export function resolveInside(rootDirectory, relativePath, label) {
  const safePath = safeRelativePath(relativePath, label);
  const resolved = path.resolve(rootDirectory, ...safePath.split("/"));
  const relative = path.relative(path.resolve(rootDirectory), resolved);
  if (relative.startsWith("..") || path.isAbsolute(relative)) {
    fail(`${label} escapes its allowed root`);
  }
  return resolved;
}

function requireString(record, key, label) {
  if (typeof record[key] !== "string" || record[key].length === 0) fail(`${label}.${key} must be a non-empty string`);
}

function requirePositiveInteger(record, key, label) {
  if (!Number.isSafeInteger(record[key]) || record[key] <= 0) fail(`${label}.${key} must be a positive safe integer`);
}

function validateAsset(asset, label, { requireFilename = true } = {}) {
  if (!isRecord(asset)) fail(`${label} must be an object`);
  const relativePath = asset.path ?? asset.filename;
  if (requireFilename) {
    if (typeof asset.filename !== "string") fail(`${label}.filename must be present`);
    safeRelativePath(asset.filename, `${label}.filename`);
  }
  if (typeof relativePath !== "string") fail(`${label}.path or filename must be present`);
  safeRelativePath(relativePath, `${label}.path`);
  requireString(asset, "version", label);
  if (/latest/i.test(asset.version)) fail(`${label}.version must be pinned, not latest`);
  isPinnedHttpsUrl(asset.sourceUrl, `${label}.sourceUrl`);
  isPinnedHttpsUrl(asset.sourceArchiveUrl, `${label}.sourceArchiveUrl`);
  if (!sha256Hex(asset.sha256)) fail(`${label}.sha256 must be lowercase SHA-256`);
  requirePositiveInteger(asset, "sizeBytes", label);
  if (!sha256Hex(asset.sourceArchiveSha256)) fail(`${label}.sourceArchiveSha256 must be lowercase SHA-256`);
  requirePositiveInteger(asset, "sourceArchiveSizeBytes", label);
  if (asset.archiveMember !== null && typeof asset.archiveMember !== "string") {
    fail(`${label}.archiveMember must be null or a string`);
  }
  if (typeof asset.archiveMember === "string") safeRelativePath(asset.archiveMember, `${label}.archiveMember`);
  requireString(asset, "licenseId", label);
  safeRelativePath(asset.licenseFile, `${label}.licenseFile`);
  return relativePath;
}

export function validateLock(lock, repositoryRoot) {
  if (!isRecord(lock)) fail("lock document must be an object");
  if (lock.schemaVersion !== 1) fail("schemaVersion must be 1");
  if (lock.targetTriple !== TARGET_TRIPLE) fail(`targetTriple must be ${TARGET_TRIPLE}`);
  if (!Array.isArray(lock.components)) fail("components must be an array");

  if (lock.status === "unpopulated") {
    if (lock.lockDigest !== null) fail("an unpopulated lock must have a null lockDigest");
    if (lock.components.length !== 0) fail("an unpopulated lock must not contain guessed component metadata");
    return { populated: false, components: [] };
  }

  if (lock.status !== "ready") fail("status must be either unpopulated or ready");
  if (!sha256Hex(lock.lockDigest)) fail("ready lockDigest must be lowercase SHA-256");
  if (lock.lockDigest !== digestLock(lock)) fail("lockDigest does not match canonical lock contents");
  if (lock.components.length !== REQUIRED_COMPONENTS.length) {
    fail(`ready lock must contain exactly ${REQUIRED_COMPONENTS.length} components`);
  }

  const names = new Set();
  const paths = new Set();
  for (const component of lock.components) {
    if (!isRecord(component)) fail("every component must be an object");
    requireString(component, "name", "component");
    if (!REQUIRED_COMPONENTS.includes(component.name)) fail(`unknown component ${component.name}`);
    if (names.has(component.name)) fail(`duplicate component ${component.name}`);
    names.add(component.name);
    const relativePath = validateAsset(component, `component ${component.name}`);
    if (!relativePath.endsWith(`-${TARGET_TRIPLE}.exe`)) fail(`component ${component.name} filename must carry the target triple`);
    if (paths.has(relativePath)) fail(`duplicate installed asset path ${relativePath}`);
    paths.add(relativePath);
    if (!Array.isArray(component.loadedAssets)) fail(`component ${component.name}.loadedAssets must be an array`);
    for (const [index, loadedAsset] of component.loadedAssets.entries()) {
      const loadedPath = validateAsset(loadedAsset, `component ${component.name}.loadedAssets[${index}]`, { requireFilename: false });
      if (paths.has(loadedPath)) fail(`duplicate installed asset path ${loadedPath}`);
      paths.add(loadedPath);
    }
  }
  for (const requiredName of REQUIRED_COMPONENTS) {
    if (!names.has(requiredName)) fail(`missing component ${requiredName}`);
  }

  for (const component of lock.components) {
    const licensePath = resolveInside(repositoryRoot, component.licenseFile, `${component.name}.licenseFile`);
    void licensePath;
    for (const asset of component.loadedAssets) {
      const loadedLicensePath = resolveInside(repositoryRoot, asset.licenseFile, `${component.name}.loadedAssets licenseFile`);
      void loadedLicensePath;
    }
  }
  return { populated: true, components: lock.components };
}

export async function readLock(repositoryRoot) {
  const lockPath = path.join(repositoryRoot, LOCK_RELATIVE_PATH);
  let raw;
  try {
    raw = await readFile(lockPath, "utf8");
  } catch (error) {
    fail(`cannot read ${LOCK_RELATIVE_PATH}: ${error.message}`);
  }
  let lock;
  try {
    lock = JSON.parse(raw);
  } catch (error) {
    fail(`${LOCK_RELATIVE_PATH} is not valid JSON: ${error.message}`);
  }
  return { lock, lockPath, validation: validateLock(lock, repositoryRoot) };
}

export async function sha256File(filePath) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(filePath)) hash.update(chunk);
  return hash.digest("hex");
}

export function randomSuffix() {
  return randomBytes(12).toString("hex");
}
