import assert from "node:assert/strict";
import test from "node:test";

import {
  REQUIRED_COMPONENTS,
  digestLock,
  validateLock,
} from "./youtube-helper-lock.mjs";

const repositoryRoot = process.cwd();

function makeAsset(name, archiveMember = null, role = null) {
  const filename = `${name}-x86_64-pc-windows-msvc.exe`;
  const ejsVersion = role === "yt-dlp" ? "0.8.0" : null;
  const ffmpegBuildId = role === "ffmpeg" ? "ffmpeg-test-build-1" : null;
  const asset = {
    name,
    version: "1.0.0",
    filename,
    path: filename,
    sourceUrl: `https://example.com/${name}/1.0.0/${name}${archiveMember === null ? ".exe" : ".zip"}`,
    sourceArchiveUrl: `https://example.com/${name}/1.0.0/${name}-source.tar.gz`,
    sha256: "a".repeat(64),
    sizeBytes: 123,
    sourceArchiveSha256: "b".repeat(64),
    sourceArchiveSizeBytes: 456,
    archiveMember,
    archiveFormat: archiveMember === null ? null : "zip",
    sourceArchiveFormat: "tar.gz",
    compatibility: {
      ytDlpEjsVersion: ejsVersion,
      ffmpegBuildId,
    },
    loadedAssets: [],
    licenseId: "MIT",
    licenseFile: `docs/third-party/${name}.LICENSE.txt`,
    sourceRecord: {
      projectUrl: `https://example.com/${name}`,
      releaseUrl: `https://example.com/${name}/releases/1.0.0`,
      revision: "v1.0.0",
      assetUrl: `https://example.com/${name}/1.0.0/${name}${archiveMember === null ? ".exe" : ".zip"}`,
      assetSha256: "a".repeat(64),
      assetSizeBytes: 123,
      archiveUrl: `https://example.com/${name}/1.0.0/${name}-source.tar.gz`,
      archiveSha256: "b".repeat(64),
      archiveSizeBytes: 456,
    },
    licenseRecord: {
      spdxExpression: "MIT",
      url: "https://spdx.org/licenses/MIT.html",
      file: `docs/third-party/${name}.LICENSE.txt`,
      sha256: "d".repeat(64),
    },
    noticeRecord: {
      url: "https://example.com/notices",
      file: "docs/third-party/THIRD_PARTY_NOTICES.md",
      sha256: "e".repeat(64),
    },
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
    components: REQUIRED_COMPONENTS.map((name) => makeAsset(name, name === archivedComponent ? `${name}/bin/${name}.exe` : null, name)),
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

test("filename and path must agree exactly", () => {
  const lock = makeLock();
  lock.components[0].path = "different.exe";
  lock.lockDigest = digestLock(lock);
  assert.throws(
    () => validateLock(lock, repositoryRoot),
    /component yt-dlp\.path must exactly match component yt-dlp\.filename/,
  );
});

test("archives require an explicit format and direct assets require null", () => {
  const archived = makeLock("evidence", "deno");
  archived.components[1].archiveFormat = null;
  archived.lockDigest = digestLock(archived);
  assert.throws(() => validateLock(archived, repositoryRoot), /component deno\.archiveFormat must be one of/);

  const direct = makeLock();
  direct.components[0].archiveFormat = "zip";
  direct.lockDigest = digestLock(direct);
  assert.throws(() => validateLock(direct, repositoryRoot), /component yt-dlp\.archiveFormat must be null/);

  const missingSourceFormat = makeLock();
  missingSourceFormat.components[0].sourceArchiveFormat = "rar";
  missingSourceFormat.lockDigest = digestLock(missingSourceFormat);
  assert.throws(() => validateLock(missingSourceFormat, repositoryRoot), /component yt-dlp\.sourceArchiveFormat must be one of/);
});

test("compatibility fields are typed and role requirements are fail-closed", () => {
  const lock = makeLock();
  lock.components[0].compatibility.ytDlpEjsVersion = 0.8;
  lock.lockDigest = digestLock(lock);
  assert.throws(() => validateLock(lock, repositoryRoot), /ytDlpEjsVersion must be a non-empty string or null/);

  const missingEjs = makeLock();
  missingEjs.components[0].compatibility.ytDlpEjsVersion = null;
  missingEjs.lockDigest = digestLock(missingEjs);
  assert.throws(() => validateLock(missingEjs, repositoryRoot), /ytDlpEjsVersion is required for yt-dlp/);

  const missingFfmpegBuild = makeLock();
  missingFfmpegBuild.components[2].compatibility.ffmpegBuildId = null;
  missingFfmpegBuild.lockDigest = digestLock(missingFfmpegBuild);
  assert.throws(() => validateLock(missingFfmpegBuild, repositoryRoot), /ffmpegBuildId is required for ffmpeg/);

  const mismatchedLoadedAsset = makeLock();
  const loadedAsset = makeAsset("yt-dlp-ejs");
  loadedAsset.compatibility.ytDlpEjsVersion = "0.7.0";
  mismatchedLoadedAsset.components[0].loadedAssets.push(loadedAsset);
  mismatchedLoadedAsset.lockDigest = digestLock(mismatchedLoadedAsset);
  assert.throws(() => validateLock(mismatchedLoadedAsset, repositoryRoot), /loadedAssets compatibility\.ytDlpEjsVersion must match/);
});

test("source, license, and notice records must bind exact provenance", () => {
  const sourceMismatch = makeLock();
  sourceMismatch.components[0].sourceRecord.assetSha256 = "f".repeat(64);
  sourceMismatch.lockDigest = digestLock(sourceMismatch);
  assert.throws(() => validateLock(sourceMismatch, repositoryRoot), /sourceRecord\.assetSha256 must exactly match/);

  const missingNotice = makeLock();
  delete missingNotice.components[0].noticeRecord;
  missingNotice.lockDigest = digestLock(missingNotice);
  assert.throws(() => validateLock(missingNotice, repositoryRoot), /noticeRecord must be an object/);

  const badLicenseHash = makeLock();
  badLicenseHash.components[0].licenseRecord.sha256 = "not-a-sha";
  badLicenseHash.lockDigest = digestLock(badLicenseHash);
  assert.throws(() => validateLock(badLicenseHash, repositoryRoot), /licenseRecord\.sha256 must be lowercase SHA-256/);
});
