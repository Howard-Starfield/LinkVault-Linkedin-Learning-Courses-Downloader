import { createHash, randomBytes } from "node:crypto";
import { createReadStream } from "node:fs";
import { readFile } from "node:fs/promises";
import path from "node:path";

export const REQUIRED_COMPONENTS = Object.freeze(["yt-dlp", "deno", "ffmpeg", "ffprobe"]);
export const TARGET_TRIPLE = "x86_64-pc-windows-msvc";
export const LOCK_RELATIVE_PATH = "docs/third-party/youtube-helpers-lock.json";
export const BINARY_RELATIVE_PATH = "apps/desktop/src-tauri/binaries";
export const ARCHIVE_FORMATS = Object.freeze(["zip", "tar.gz", "tar.xz", "tar.bz2", "tar.zst"]);

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

function requirePinnedString(record, key, label) {
  requireString(record, key, label);
  if (/latest/i.test(record[key])) fail(`${label}.${key} must be pinned, not latest`);
}

function requireNullablePinnedString(record, key, label) {
  if (record[key] !== null && typeof record[key] !== "string") {
    fail(`${label}.${key} must be a non-empty string or null`);
  }
  if (typeof record[key] === "string") {
    if (record[key].length === 0) fail(`${label}.${key} must be a non-empty string or null`);
    if (/latest/i.test(record[key])) fail(`${label}.${key} must be pinned, not latest`);
  }
}

function requireArchiveFormat(value, label, { nullable = false } = {}) {
  if (nullable && value === null) return;
  if (typeof value !== "string" || !ARCHIVE_FORMATS.includes(value)) {
    fail(`${label} must be one of ${ARCHIVE_FORMATS.join(", ")}${nullable ? " or null" : ""}`);
  }
}

function requireMatchingString(record, key, expected, label) {
  requireString(record, key, label);
  if (record[key] !== expected) fail(`${label}.${key} must exactly match ${JSON.stringify(expected)}`);
}

function validateCompatibility(asset, label, role) {
  if (!isRecord(asset.compatibility)) fail(`${label}.compatibility must be an object`);
  requireNullablePinnedString(asset.compatibility, "ytDlpEjsVersion", `${label}.compatibility`);
  requireNullablePinnedString(asset.compatibility, "ffmpegBuildId", `${label}.compatibility`);
  if (role === "yt-dlp" && asset.compatibility.ytDlpEjsVersion === null) {
    fail(`${label}.compatibility.ytDlpEjsVersion is required for yt-dlp`);
  }
  if ((role === "ffmpeg" || role === "ffprobe") && asset.compatibility.ffmpegBuildId === null) {
    fail(`${label}.compatibility.ffmpegBuildId is required for ${role}`);
  }
}

function validateProvenance(asset, label) {
  if (!isRecord(asset.sourceRecord)) fail(`${label}.sourceRecord must be an object`);
  isPinnedHttpsUrl(asset.sourceRecord.projectUrl, `${label}.sourceRecord.projectUrl`);
  isPinnedHttpsUrl(asset.sourceRecord.releaseUrl, `${label}.sourceRecord.releaseUrl`);
  requirePinnedString(asset.sourceRecord, "revision", `${label}.sourceRecord`);
  requireMatchingString(asset.sourceRecord, "assetUrl", asset.sourceUrl, `${label}.sourceRecord`);
  requireMatchingString(asset.sourceRecord, "assetSha256", asset.sha256, `${label}.sourceRecord`);
  if (asset.sourceRecord.assetSizeBytes !== asset.sizeBytes) {
    fail(`${label}.sourceRecord.assetSizeBytes must exactly match ${label}.sizeBytes`);
  }
  requireMatchingString(asset.sourceRecord, "archiveUrl", asset.sourceArchiveUrl, `${label}.sourceRecord`);
  requireMatchingString(asset.sourceRecord, "archiveSha256", asset.sourceArchiveSha256, `${label}.sourceRecord`);
  if (asset.sourceRecord.archiveSizeBytes !== asset.sourceArchiveSizeBytes) {
    fail(`${label}.sourceRecord.archiveSizeBytes must exactly match ${label}.sourceArchiveSizeBytes`);
  }

  if (!isRecord(asset.licenseRecord)) fail(`${label}.licenseRecord must be an object`);
  requireMatchingString(asset.licenseRecord, "spdxExpression", asset.licenseId, `${label}.licenseRecord`);
  isPinnedHttpsUrl(asset.licenseRecord.url, `${label}.licenseRecord.url`);
  requireMatchingString(asset.licenseRecord, "file", asset.licenseFile, `${label}.licenseRecord`);
  if (!sha256Hex(asset.licenseRecord.sha256)) fail(`${label}.licenseRecord.sha256 must be lowercase SHA-256`);

  if (!isRecord(asset.noticeRecord)) fail(`${label}.noticeRecord must be an object`);
  safeRelativePath(asset.noticeRecord.file, `${label}.noticeRecord.file`);
  isPinnedHttpsUrl(asset.noticeRecord.url, `${label}.noticeRecord.url`);
  if (!sha256Hex(asset.noticeRecord.sha256)) fail(`${label}.noticeRecord.sha256 must be lowercase SHA-256`);
}

function validateAsset(asset, label, { role = null } = {}) {
  if (!isRecord(asset)) fail(`${label} must be an object`);
  if (typeof asset.filename !== "string") fail(`${label}.filename must be present`);
  const filename = safeRelativePath(asset.filename, `${label}.filename`);
  if (typeof asset.path !== "string") fail(`${label}.path must be present and match filename`);
  const assetPath = safeRelativePath(asset.path, `${label}.path`);
  if (assetPath !== filename) fail(`${label}.path must exactly match ${label}.filename`);
  const relativePath = filename;
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
  if (typeof asset.archiveMember === "string") {
    safeRelativePath(asset.archiveMember, `${label}.archiveMember`);
    requireArchiveFormat(asset.archiveFormat, `${label}.archiveFormat`);
  } else {
    if (asset.archiveFormat !== null) fail(`${label}.archiveFormat must be null when archiveMember is null`);
  }
  requireArchiveFormat(asset.sourceArchiveFormat, `${label}.sourceArchiveFormat`);
  if (asset.archiveMember === null) {
    if (asset.distributionArchiveSizeBytes !== undefined || asset.distributionArchiveSha256 !== undefined) {
      fail(`${label}.distributionArchive* is only valid when archiveMember is set`);
    }
  } else {
    requirePositiveInteger(asset, "distributionArchiveSizeBytes", label);
    if (!sha256Hex(asset.distributionArchiveSha256)) {
      fail(`${label}.distributionArchiveSha256 must be lowercase SHA-256`);
    }
  }
  requireString(asset, "licenseId", label);
  safeRelativePath(asset.licenseFile, `${label}.licenseFile`);
  validateCompatibility(asset, label, role);
  validateProvenance(asset, label);
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

  if (lock.status !== "evidence" && lock.status !== "ready") {
    fail("status must be unpopulated, evidence, or ready");
  }
  if (!sha256Hex(lock.lockDigest)) fail(`${lock.status} lockDigest must be lowercase SHA-256`);
  if (lock.lockDigest !== digestLock(lock)) fail("lockDigest does not match canonical lock contents");
  if (lock.components.length !== REQUIRED_COMPONENTS.length) {
    fail(`${lock.status} lock must contain exactly ${REQUIRED_COMPONENTS.length} components`);
  }

  const names = new Set();
  const paths = new Set();
  for (const component of lock.components) {
    if (!isRecord(component)) fail("every component must be an object");
    requireString(component, "name", "component");
    if (!REQUIRED_COMPONENTS.includes(component.name)) fail(`unknown component ${component.name}`);
    if (names.has(component.name)) fail(`duplicate component ${component.name}`);
    names.add(component.name);
    const relativePath = validateAsset(component, `component ${component.name}`, { role: component.name });
    const expectedPath = `${component.name}-${TARGET_TRIPLE}.exe`;
    if (relativePath !== expectedPath) {
      fail(`component ${component.name} path must be exactly ${expectedPath}`);
    }
    if (paths.has(relativePath)) fail(`duplicate installed asset path ${relativePath}`);
    paths.add(relativePath);
    if (!Array.isArray(component.loadedAssets)) fail(`component ${component.name}.loadedAssets must be an array`);
    for (const [index, loadedAsset] of component.loadedAssets.entries()) {
      const loadedPath = validateAsset(loadedAsset, `component ${component.name}.loadedAssets[${index}]`);
      if (paths.has(loadedPath)) fail(`duplicate installed asset path ${loadedPath}`);
      paths.add(loadedPath);
    }
  }
  for (const requiredName of REQUIRED_COMPONENTS) {
    if (!names.has(requiredName)) fail(`missing component ${requiredName}`);
  }

  for (const component of lock.components) {
    for (const loadedAsset of component.loadedAssets) {
      for (const key of ["ytDlpEjsVersion", "ffmpegBuildId"]) {
        if (loadedAsset.compatibility[key] !== null && loadedAsset.compatibility[key] !== component.compatibility[key]) {
          fail(`component ${component.name}.loadedAssets compatibility.${key} must match the component`);
        }
      }
    }
    if (component.name === "ffmpeg" || component.name === "ffprobe") {
      const buildId = component.compatibility.ffmpegBuildId;
      const peer = lock.components.find((candidate) => candidate.name === (component.name === "ffmpeg" ? "ffprobe" : "ffmpeg"));
      if (peer && peer.compatibility.ffmpegBuildId !== buildId) {
        fail(`components ffmpeg and ffprobe must share one ffmpegBuildId`);
      }
    }
    const licensePath = resolveInside(repositoryRoot, component.licenseFile, `${component.name}.licenseFile`);
    const licenseRecordPath = resolveInside(repositoryRoot, component.licenseRecord.file, `${component.name}.licenseRecord.file`);
    const noticePath = resolveInside(repositoryRoot, component.noticeRecord.file, `${component.name}.noticeRecord.file`);
    void licensePath;
    void licenseRecordPath;
    void noticePath;
    for (const asset of component.loadedAssets) {
      const loadedLicensePath = resolveInside(repositoryRoot, asset.licenseFile, `${component.name}.loadedAssets licenseFile`);
      const loadedLicenseRecordPath = resolveInside(
        repositoryRoot,
        asset.licenseRecord.file,
        `${component.name}.loadedAssets licenseRecord.file`,
      );
      const loadedNoticePath = resolveInside(
        repositoryRoot,
        asset.noticeRecord.file,
        `${component.name}.loadedAssets noticeRecord.file`,
      );
      void loadedLicensePath;
      void loadedLicenseRecordPath;
      void loadedNoticePath;
    }
  }
  return { populated: lock.status === "ready", components: lock.components };
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
