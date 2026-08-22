import assert from "node:assert/strict";
import test from "node:test";

import {
  REQUIRED_COMPONENTS,
  digestLock,
  validateLock,
} from "./youtube-helper-lock.mjs";

const repositoryRoot = process.cwd();

function makeAsset(name, archiveMember = null) {
  const asset = {
    name,
    version: "1.0.0",
    filename: `${name}-x86_64-pc-windows-msvc.exe`,
    sourceUrl: `https://example.com/${name}/1.0.0/${name}${archiveMember === null ? ".exe" : ".zip"}`,
    sourceArchiveUrl: `https://example.com/${name}/1.0.0/${name}-source.tar.gz`,
    sha256: "a".repeat(64),
    sizeBytes: 123,
    sourceArchiveSha256: "b".repeat(64),
    sourceArchiveSizeBytes: 456,
    archiveMember,
    loadedAssets: [],
    licenseId: "MIT",
    licenseFile: `docs/third-party/${name}.LICENSE.txt`,
  };
  if (archiveMember !== null) {
    asset.distributionArchiveSha256 = "c".repeat(64);
    asset.distributionArchiveSizeBytes = 789;
  }
  return asset;
}

function makeLock(status = "evidence", archivedComponent = null) {
  const lock = {
    schemaVersion: 1,
    targetTriple: "x86_64-pc-windows-msvc",
    status,
    lockDigest: null,
    components: REQUIRED_COMPONENTS.map((name) => makeAsset(name, name === archivedComponent ? `${name}/bin/${name}.exe` : null)),
  };
  lock.lockDigest = digestLock(lock);
  return lock;
}

test("evidence lock validates but remains non-executable", () => {
  const result = validateLock(makeLock(), repositoryRoot);
  assert.equal(result.populated, false);
  assert.equal(result.components.length, REQUIRED_COMPONENTS.length);

  const ready = validateLock(makeLock("ready"), repositoryRoot);
  assert.equal(ready.populated, true);
});

test("unpopulated lock remains empty and non-executable", () => {
  const lock = {
    schemaVersion: 1,
    targetTriple: "x86_64-pc-windows-msvc",
    status: "unpopulated",
    lockDigest: null,
    components: [],
  };
  assert.equal(validateLock(lock, repositoryRoot).populated, false);
});

test("archived assets require distribution archive size and digest", () => {
  const lock = makeLock("evidence", "deno");
  assert.doesNotThrow(() => validateLock(lock, repositoryRoot));

  delete lock.components[1].distributionArchiveSizeBytes;
  lock.lockDigest = digestLock(lock);
  assert.throws(
    () => validateLock(lock, repositoryRoot),
    /component deno\.distributionArchiveSizeBytes must be a positive safe integer/,
  );
});

test("direct executable assets reject archive-only metadata", () => {
  const lock = makeLock();
  lock.components[0].distributionArchiveSizeBytes = 789;
  lock.components[0].distributionArchiveSha256 = "c".repeat(64);
  lock.lockDigest = digestLock(lock);
  assert.throws(
    () => validateLock(lock, repositoryRoot),
    /component yt-dlp\.distributionArchive\* is only valid when archiveMember is set/,
  );
});
