import { createHash } from "node:crypto";
import { createWriteStream } from "node:fs";
import { execFile as execFileCallback } from "node:child_process";
import { lstat, mkdir, mkdtemp, rename, rm, stat } from "node:fs/promises";
import { promisify } from "node:util";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  BINARY_RELATIVE_PATH,
  fail,
  randomSuffix,
  readLock,
  resolveInside,
  sha256File,
} from "./youtube-helper-lock.mjs";

const execFile = promisify(execFileCallback);
const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const desktopDirectory = path.resolve(scriptDirectory, "..");
const repositoryRoot = path.resolve(desktopDirectory, "../..");
const binaryDirectory = resolveInside(repositoryRoot, BINARY_RELATIVE_PATH, "binary directory");
const maximumDownloadBytes = 4 * 1024 * 1024 * 1024;

async function downloadToFile(url, destination, expectedSize, label) {
  if (typeof fetch !== "function") fail("this Node runtime does not provide fetch");
  const response = await fetch(url, { redirect: "error" });
  if (!response.ok) fail(`${label} download returned HTTP ${response.status}`);
  const advertisedLength = response.headers.get("content-length");
  if (advertisedLength !== null && Number(advertisedLength) !== expectedSize) {
    fail(`${label} content-length mismatch before promotion`);
  }
  if (response.body === null) fail(`${label} response had no body`);
  const output = createWriteStream(destination, { flags: "wx" });
  const hash = createHash("sha256");
  const reader = response.body.getReader();
  let total = 0;
  try {
    while (true) {
      const next = await reader.read();
      if (next.done) break;
      const chunk = Buffer.from(next.value);
      total += chunk.byteLength;
      if (total > maximumDownloadBytes) fail(`${label} exceeds the bounded download size`);
      hash.update(chunk);
      if (!output.write(chunk)) await new Promise((resolve) => output.once("drain", resolve));
    }
    await new Promise((resolve, reject) => {
      output.once("error", reject);
      output.end(resolve);
    });
  } catch (error) {
    output.destroy();
    throw error;
  }
  if (total !== expectedSize) fail(`${label} size mismatch: expected ${expectedSize}, got ${total}`);
  return hash.digest("hex");
}

function archiveMemberPath(root, member, label) {
  const normalized = member.replaceAll("\\", "/");
  const resolved = resolveInside(root, normalized, label);
  return resolved;
}

async function extractArchiveMember(archivePath, member, extractionRoot, label) {
  const listing = await execFile("tar", ["-tf", archivePath], { encoding: "utf8", windowsHide: true, maxBuffer: 16 * 1024 * 1024 });
  const members = listing.stdout
    .split(/\r?\n/)
    .map((entry) => entry.trim().replaceAll("\\", "/").replace(/\/$/, ""))
    .filter(Boolean);
  for (const listedMember of members) archiveMemberPath(extractionRoot, listedMember, `${label} archive member`);
  const normalizedMember = member.replaceAll("\\", "/");
  if (!members.includes(normalizedMember)) fail(`${label} archive does not contain exact member ${normalizedMember}`);
  const verboseListing = await execFile("tar", ["-tvf", archivePath], { encoding: "utf8", windowsHide: true, maxBuffer: 16 * 1024 * 1024 });
  for (const line of verboseListing.stdout.split(/\r?\n/).map((entry) => entry.trim()).filter(Boolean)) {
    const type = line[0];
    if (type !== "-" && type !== "d") fail(`${label} archive contains a non-regular entry`);
  }
  await mkdir(extractionRoot, { recursive: true });
  await execFile("tar", ["-xf", archivePath, "-C", extractionRoot, normalizedMember], {
    encoding: "utf8",
    windowsHide: true,
    maxBuffer: 16 * 1024 * 1024,
  });
  const extracted = archiveMemberPath(extractionRoot, normalizedMember, `${label} extracted member`);
  const extractedStats = await lstat(extracted);
  if (extractedStats.isSymbolicLink()) fail(`${label} archive member is a symbolic link`);
  if (!extractedStats.isFile()) fail(`${label} archive member is not a regular file`);
  return extracted;
}

async function verifySourceArchive(asset, archivePath, label) {
  const sourceHash = await sha256File(archivePath);
  const sourceStats = await stat(archivePath);
  if (sourceStats.size !== asset.sourceArchiveSizeBytes) fail(`${label} source archive size mismatch`);
  if (sourceHash !== asset.sourceArchiveSha256) fail(`${label} source archive SHA-256 mismatch`);
}

async function stageAsset(asset, stagingRoot, label) {
  const relativePath = asset.path ?? asset.filename;
  const outputPath = resolveInside(stagingRoot, relativePath, `${label} output path`);
  await mkdir(path.dirname(outputPath), { recursive: true });
  const sourceDownload = path.join(stagingRoot, `.download-${randomSuffix()}`);
  const sourceSize = asset.archiveMember === null ? asset.sizeBytes : asset.distributionArchiveSizeBytes;
  const sourceHash = await downloadToFile(
    asset.sourceUrl,
    sourceDownload,
    sourceSize,
    asset.archiveMember === null ? label : `${label} distribution archive`,
  );
  if (asset.archiveMember === null && sourceHash !== asset.sha256) fail(`${label} SHA-256 mismatch`);
  if (asset.archiveMember !== null && sourceHash !== asset.distributionArchiveSha256) {
    fail(`${label} distribution archive SHA-256 mismatch`);
  }
  let executablePath = sourceDownload;
  if (asset.archiveMember !== null) {
    executablePath = await extractArchiveMember(sourceDownload, asset.archiveMember, path.join(stagingRoot, `.extract-${randomSuffix()}`), label);
  }
  const executableStats = await stat(executablePath);
  if (executableStats.size !== asset.sizeBytes) fail(`${label} extracted size mismatch`);
  const executableHash = await sha256File(executablePath);
  if (executableHash !== asset.sha256) fail(`${label} extracted SHA-256 mismatch`);
  await rename(executablePath, outputPath);

  const sourceArchivePath = path.join(stagingRoot, `.source-${randomSuffix()}`);
  const sourceArchiveHash = await downloadToFile(
    asset.sourceArchiveUrl,
    sourceArchivePath,
    asset.sourceArchiveSizeBytes,
    `${label} source archive`,
  );
  if (sourceArchiveHash !== asset.sourceArchiveSha256) fail(`${label} source archive SHA-256 mismatch`);
  await verifySourceArchive(asset, sourceArchivePath, label);
}

const { lock, validation } = await readLock(repositoryRoot);
if (!validation.populated) {
  if (lock.status === "unpopulated") {
    fail("lock is intentionally unpopulated; populate it from reviewed authoritative release metadata before fetching helpers");
  }
  fail(`lock status ${lock.status} is non-executable; helper fetching requires status ready`);
}

try {
  await lstat(binaryDirectory);
  fail(`${BINARY_RELATIVE_PATH} already exists; refusing to replace existing helper files`);
} catch (error) {
  if (error?.code !== "ENOENT") throw error;
}

const stagingParent = path.dirname(binaryDirectory);
await mkdir(stagingParent, { recursive: true });
const stagingRoot = await mkdtemp(path.join(stagingParent, `.youtube-helpers-${process.pid}-`));
const stagingOutput = path.join(stagingRoot, "binaries");
await mkdir(stagingOutput, { recursive: true });

try {
  for (const component of lock.components) {
    await stageAsset(component, stagingOutput, component.name);
    for (const [index, asset] of component.loadedAssets.entries()) {
      await stageAsset(asset, stagingOutput, `${component.name}.loadedAssets[${index}]`);
    }
  }
  await rename(stagingOutput, binaryDirectory);
  await rm(stagingRoot, { recursive: true, force: true });
  console.log(`Fetched and atomically promoted ${lock.components.length} reviewed YouTube helper components for ${lock.targetTriple}.`);
} catch (error) {
  await rm(stagingRoot, { recursive: true, force: true });
  throw error;
}
