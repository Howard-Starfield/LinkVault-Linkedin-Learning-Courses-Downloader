export function clippingSaveStatusCopy(
  status: string | undefined,
  errorCode: string | null | undefined,
  checkpointStatus: string | undefined
) {
  if (errorCode === "CLIPPING_INVALID_TITLE") return "Title must be 1–200 characters.";
  if (errorCode === "CLIPPING_NOTE_TOO_LARGE") return "Note exceeds the 2 MiB limit.";
  if (errorCode === "CLIPPING_INVALID_MARKDOWN") return "Note contains an invalid null character.";
  if (status === "saving") return "Saving…";
  if (status === "dirty") return "Unsaved changes";
  if (status === "failed" && checkpointStatus === "durable") return "Recovered draft saved locally.";
  if (status === "failed") return "Save failed. Your draft is still here.";
  if (status === "conflict") return "Changed in another window";
  return "Saved";
}
