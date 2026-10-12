import assert from "node:assert/strict";
import test from "node:test";
import { helperRecoveryKind } from "../src/lib/youtube/helperRecovery.ts";

test("missing/integrity/blocked codes need reinstall", () => {
  for (const code of ["HELPER_MISSING", "HELPER_INTEGRITY_FAILED", "HELPER_EXECUTION_BLOCKED"]) {
    assert.equal(helperRecoveryKind({ code, message: "m" }), "reinstall", code);
  }
});

test("runtime integrity failure surfaced as HELPER_FAILED needs reinstall", () => {
  assert.equal(
    helperRecoveryKind({ code: "HELPER_FAILED", message: "helper integrity validation failed: ffmpeg hash mismatch" }),
    "reinstall"
  );
});

test("helperLockDigest contract error needs reinstall, in object or string form", () => {
  assert.equal(helperRecoveryKind({ code: "MANIFEST_CONTRACT_INVALID", message: "helperLockDigest must be a lowercase SHA-256 digest" }), "reinstall");
  assert.equal(helperRecoveryKind("MANIFEST_CONTRACT_INVALID: helperLockDigest must be a lowercase SHA-256 digest"), "reinstall");
});

test("ordinary helper run failures are runtime, not reinstall", () => {
  assert.equal(helperRecoveryKind({ code: "HELPER_FAILED", message: "ERROR: [youtube] abc: Video unavailable" }), "runtime");
  assert.equal(helperRecoveryKind({ code: "HELPER_TIMEOUT", message: "yt-dlp helper timed out" }), "runtime");
  assert.equal(helperRecoveryKind({ code: "HELPER_TEMP_ROOT_INVALID", message: "temp root is a junction" }), "runtime");
});

test("media and collision failures are not helper failures unless they carry the integrity code", () => {
  assert.equal(helperRecoveryKind({ code: "MEDIA_VERIFY_FAILED", message: "FFprobe could not read the media artifact" }), null);
  assert.equal(helperRecoveryKind({ code: "OUTPUT_COLLISION", message: "existing output is not an exact verified match: x" }), null);
  // Task 1 Step 6 maps probe-time integrity failures to this code.
  assert.equal(helperRecoveryKind({ code: "HELPER_INTEGRITY_FAILED", message: "ffmpeg hash mismatch" }), "reinstall");
});

test("unrelated and malformed errors are not helper failures", () => {
  assert.equal(helperRecoveryKind({ code: "MANIFEST_CONTRACT_INVALID", message: "videoId must not be empty" }), null);
  assert.equal(helperRecoveryKind({ code: "SCAN_PLAN_STALE", message: "m" }), null);
  assert.equal(helperRecoveryKind(new Error("network down")), null);
  assert.equal(helperRecoveryKind(null), null);
  assert.equal(helperRecoveryKind(42), null);
});
