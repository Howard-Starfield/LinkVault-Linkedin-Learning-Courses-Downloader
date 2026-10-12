/** Window event carrying `detail: boolean` (true = helpers ready). */
export const YOUTUBE_HELPER_STATUS_EVENT = "linkvault:youtube-helper-status";

export type HelperRecoveryKind = "reinstall" | "runtime";

const REINSTALL_CODES: ReadonlySet<string> = new Set([
  "HELPER_MISSING",
  "HELPER_INTEGRITY_FAILED"
]);
// Helper start failures (e.g. AV/WDAC blocking CreateProcessW -> HELPER_FAILED "failed to start
// trusted helper: ...") are deliberately runtime: reinstalling a blocked exe would loop.
const RUNTIME_CODES: ReadonlySet<string> = new Set(["HELPER_FAILED", "HELPER_TIMEOUT", "HELPER_TEMP_ROOT_INVALID"]);
const INTEGRITY_MESSAGE_PREFIX = "helper integrity validation failed";
const CODE_PREFIX = /^([A-Z][A-Z0-9_]+):\s*([\s\S]*)$/;

function codeAndMessage(error: unknown): { code: string; message: string } | null {
  if (typeof error === "string") {
    const match = CODE_PREFIX.exec(error.trim());
    return match ? { code: match[1], message: match[2] } : null;
  }
  if (typeof error === "object" && error !== null && "code" in error) {
    const { code } = error;
    const message = "message" in error && typeof error.message === "string" ? error.message : "";
    return typeof code === "string" ? { code, message } : null;
  }
  return null;
}

/** How the UI should recover from an invoke or item failure, or null if it is not a helper problem. */
export function helperRecoveryKind(error: unknown): HelperRecoveryKind | null {
  const parsed = codeAndMessage(error);
  if (!parsed) return null;
  if (REINSTALL_CODES.has(parsed.code)) return "reinstall";
  if (parsed.code === "MANIFEST_CONTRACT_INVALID") {
    return parsed.message.includes("helperLockDigest") ? "reinstall" : null;
  }
  if (RUNTIME_CODES.has(parsed.code)) {
    return parsed.message.trim().startsWith(INTEGRITY_MESSAGE_PREFIX) ? "reinstall" : "runtime";
  }
  return null;
}
