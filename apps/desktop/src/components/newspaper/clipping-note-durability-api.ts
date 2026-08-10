import { invoke } from "@tauri-apps/api/core";

export type ClippingNoteCheckpointRequest = {
  clippingId: string;
  baseRevision: number;
  writerSessionId: string;
  writerSequence: number;
  title: string;
  markdown: string;
};

export type ClippingNoteCheckpointAck = Pick<
  ClippingNoteCheckpointRequest,
  "clippingId" | "writerSessionId" | "writerSequence"
>;

export type ClippingNoteRecoveryIdentity = {
  clippingId: string;
  writerSessionId: string;
  writerSequence: number;
};

export type ClippingNoteRecoveryResponse = {
  status: "none" | "matching" | "canonical_changed" | "invalid";
  canonicalRevision: number;
  identity: ClippingNoteRecoveryIdentity | null;
  draft: {
    baseRevision: number;
    title: string;
    markdown: string;
    updatedAt: number;
  } | null;
};

type DurabilityHarness = {
  checkpoint?: (request: ClippingNoteCheckpointRequest) => Promise<ClippingNoteCheckpointAck>;
  loadRecovery?: (clippingId: string) => Promise<ClippingNoteRecoveryResponse>;
  claimRecovery?: (request: {
    clippingId: string;
    priorWriterSessionId: string;
    priorWriterSequence: number;
    writerSessionId: string;
  }) => Promise<ClippingNoteRecoveryResponse>;
  discardRecovery?: (identity: ClippingNoteRecoveryIdentity) => Promise<void>;
};

function harness() {
  if (typeof window === "undefined" || window.location.hostname !== "127.0.0.1") return undefined;
  return (window as Window & { __NEWSPAPER_CLIPPINGS_API__?: DurabilityHarness })
    .__NEWSPAPER_CLIPPINGS_API__;
}

export function checkpointClippingNote(request: ClippingNoteCheckpointRequest) {
  const write = harness()?.checkpoint;
  if (write) return write(request);
  return invoke<ClippingNoteCheckpointAck>("checkpoint_newspaper_clipping_note", { request });
}

export function loadClippingNoteRecovery(clippingId: string) {
  const load = harness()?.loadRecovery;
  if (load) return load(clippingId);
  return invoke<ClippingNoteRecoveryResponse>("load_newspaper_clipping_note_recovery", {
    request: { clippingId }
  });
}

export function claimClippingNoteRecovery(
  clippingId: string,
  prior: ClippingNoteRecoveryIdentity,
  writerSessionId: string
) {
  const request = {
    clippingId,
    priorWriterSessionId: prior.writerSessionId,
    priorWriterSequence: prior.writerSequence,
    writerSessionId
  };
  const claim = harness()?.claimRecovery;
  if (claim) return claim(request);
  return invoke<ClippingNoteRecoveryResponse>("claim_newspaper_clipping_note_recovery", {
    request
  });
}

export function discardClippingNoteRecovery(identity: ClippingNoteRecoveryIdentity) {
  const discard = harness()?.discardRecovery;
  if (discard) return discard(identity);
  return invoke<void>("discard_newspaper_clipping_note_recovery", { request: identity });
}
