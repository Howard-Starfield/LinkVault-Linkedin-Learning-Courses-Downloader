import { useCallback, useEffect, useRef, useState } from "react";
import {
  checkpointClippingNote,
  claimClippingNoteRecovery,
  discardClippingNoteRecovery,
  loadClippingNoteRecovery,
  type ClippingNoteRecoveryResponse
} from "./clipping-note-durability-api";
import {
  ClippingNoteCheckpointController,
  type ClippingNoteCheckpointView
} from "./clipping-note-checkpoint-controller";
import {
  ClippingNoteSaveController,
  type ClippingNoteDocument,
  type ClippingNoteSaveView
} from "./clipping-note-save-controller";
import {
  clippingErrorCode,
  updateNewspaperClipping,
  type NewspaperClippingDetail
} from "./newspaper-api";

type Runtime = {
  save: ClippingNoteSaveController;
  checkpoint: ClippingNoteCheckpointController;
  writerSessionId: string;
};
function documentFromDetail(detail: NewspaperClippingDetail): ClippingNoteDocument {
  return {
    documentId: detail.id,
    title: detail.title,
    markdown: detail.noteMarkdown,
    revision: detail.revision
  };
}
function validateCanonical({ title, markdown }: { title: string; markdown: string }) {
  const trimmed = title.trim();
  if (!trimmed || [...trimmed].length > 200 || new TextEncoder().encode(trimmed).length > 800) {
    return "CLIPPING_INVALID_TITLE";
  }
  if (markdown.includes("\0")) return "CLIPPING_INVALID_MARKDOWN";
  if (new TextEncoder().encode(markdown).length > 2_097_152) return "CLIPPING_NOTE_TOO_LARGE";
  return null;
}
function validateRecovery({ title, markdown }: { title: string; markdown: string }) {
  if (title.includes("\0") || markdown.includes("\0")) return "CLIPPING_RECOVERY_INVALID";
  if (title.length <= 1_365 && markdown.length <= 1_398_101) return null;
  const encoder = new TextEncoder();
  if (encoder.encode(title).length > 4_096 || encoder.encode(markdown).length > 4_194_304) {
    return "CLIPPING_RECOVERY_TOO_LARGE";
  }
  return null;
}

async function flushRuntime(runtime: Runtime) {
  if (await runtime.save.flush()) return true;
  return runtime.checkpoint.ensureDurable();
}

export function useClippingNoteDurability({
  detail,
  onSaved,
  registerFlush
}: {
  detail: NewspaperClippingDetail;
  onSaved: (detail: NewspaperClippingDetail) => void;
  registerFlush: (flush: (() => Promise<boolean>) | null) => void;
}) {
  const runtimeRef = useRef<Runtime | null>(null);
  const onSavedRef = useRef(onSaved);
  const [saveView, setSaveView] = useState<ClippingNoteSaveView | null>(null);
  const [checkpointView, setCheckpointView] = useState<ClippingNoteCheckpointView | null>(null);
  const [recovery, setRecovery] = useState<ClippingNoteRecoveryResponse | null>(null);
  const [initializationError, setInitializationError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  onSavedRef.current = onSaved;

  useEffect(() => {
    let cancelled = false;
    let unsubscribeSave: (() => void) | null = null;
    const writerSessionId = crypto.randomUUID();
    runtimeRef.current = null;
    setSaveView(null);
    setCheckpointView(null);
    setRecovery(null);
    setInitializationError(null);
    registerFlush(async () => false);

    void (async () => {
      try {
        let loaded = await loadClippingNoteRecovery(detail.id);
        if (cancelled) return;
        if (loaded.status === "invalid") {
          setRecovery(loaded);
          return;
        }
        if (loaded.status !== "none") {
          if (!loaded.identity || !loaded.draft) throw new Error("CLIPPING_RECOVERY_INVALID");
          loaded = await claimClippingNoteRecovery(detail.id, loaded.identity, writerSessionId);
          if (cancelled) return;
        }

        const checkpoint = new ClippingNoteCheckpointController(
          detail.id,
          writerSessionId,
          detail.revision,
          detail.title,
          detail.noteMarkdown,
          async (request) => {
            const acknowledged = await checkpointClippingNote({
              clippingId: request.documentId,
              baseRevision: request.baseRevision,
              writerSessionId: request.writerSessionId,
              writerSequence: request.writerSequence,
              title: request.title,
              markdown: request.markdown
            });
            return { ...request, documentId: acknowledged.clippingId };
          },
          clippingErrorCode,
          500,
          2_000,
          validateRecovery
        );
        if (loaded.status !== "none" && loaded.identity && loaded.draft) {
          checkpoint.restoreDurable(
            loaded.draft.baseRevision,
            loaded.draft.title,
            loaded.draft.markdown,
            loaded.identity.writerSequence
          );
        }
        const save = new ClippingNoteSaveController(
          documentFromDetail(detail),
          async (request) => {
            checkpoint.setDraft(request.expectedRevision, request.title, request.markdown);
            const submittedSequence = checkpoint.getSnapshot().writerSequence;
            if (!(await checkpoint.ensureDurable(submittedSequence))) {
              throw new Error(checkpoint.getSnapshot().errorCode ?? "CLIPPING_RECOVERY_FAILED");
            }
            const updated = await updateNewspaperClipping({
              clippingId: request.documentId,
              expectedRevision: request.expectedRevision,
              title: request.title,
              noteMarkdown: request.markdown,
              checkpoint: { writerSessionId, writerSequence: submittedSequence }
            });
            checkpoint.acknowledgeCanonicalSave(writerSessionId, submittedSequence, updated.revision);
            setRecovery(null);
            onSavedRef.current(updated);
            return documentFromDetail(updated);
          },
          clippingErrorCode,
          800,
          validateCanonical
        );
        unsubscribeSave = save.subscribe(setSaveView);
        if (loaded.status === "matching" && loaded.draft) {
          save.setDraft(loaded.draft.title, loaded.draft.markdown);
          setRecovery(loaded);
        } else if (loaded.status === "canonical_changed" && loaded.draft) {
          save.restoreConflict(loaded.draft.title, loaded.draft.markdown);
          setRecovery(loaded);
        }
        const runtime = { save, checkpoint, writerSessionId };
        runtimeRef.current = runtime;
        setSaveView(save.getSnapshot());
        setCheckpointView(checkpoint.getSnapshot());
        registerFlush(() => flushRuntime(runtime));
      } catch (error) {
        if (!cancelled) setInitializationError(clippingErrorCode(error));
      }
    })();

    return () => {
      cancelled = true;
      registerFlush(null);
      unsubscribeSave?.();
      const runtime = runtimeRef.current;
      runtime?.save.dispose();
      runtime?.checkpoint.dispose();
      runtimeRef.current = null;
    };
  }, [detail.id, registerFlush, reload]);
  useEffect(() => {
    const flushOnBlur = () => {
      if (runtimeRef.current) void flushRuntime(runtimeRef.current);
    };
    window.addEventListener("blur", flushOnBlur);
    return () => window.removeEventListener("blur", flushOnBlur);
  }, []);
  const setTitle = useCallback((title: string) => {
    const runtime = runtimeRef.current;
    if (!runtime) return;
    const save = runtime.save.getSnapshot();
    runtime.checkpoint.setDraft(save.persistedRevision, title, save.draftMarkdown);
    runtime.save.setTitle(title);
  }, []);
  const setMarkdown = useCallback((markdown: string) => {
    const runtime = runtimeRef.current;
    if (!runtime) return;
    const save = runtime.save.getSnapshot();
    runtime.checkpoint.setDraft(save.persistedRevision, save.draftTitle, markdown);
    runtime.save.setMarkdown(markdown);
  }, []);
  const useSavedVersion = useCallback(async (latest: NewspaperClippingDetail) => {
    const runtime = runtimeRef.current;
    if (!runtime) return false;
    try {
      const checkpoint = runtime.checkpoint.getSnapshot();
      if (checkpoint.writerSequence > 0) {
        await discardClippingNoteRecovery({
          clippingId: detail.id,
          writerSessionId: runtime.writerSessionId,
          writerSequence: checkpoint.writerSequence
        });
      }
      runtime.checkpoint.resetCanonical(latest.revision, latest.title, latest.noteMarkdown);
      runtime.save.useSavedVersion(documentFromDetail(latest));
      setRecovery(null);
      onSavedRef.current(latest);
      return true;
    } catch (error) {
      setInitializationError(clippingErrorCode(error));
      return false;
    }
  }, [detail.id]);
  const discardInvalidRecovery = useCallback(async () => {
    if (!recovery?.identity) return false;
    return discardClippingNoteRecovery(recovery.identity).then(() => {
      setReload((current) => current + 1);
      return true;
    }, (error) => {
      setInitializationError(clippingErrorCode(error));
      return false;
    });
  }, [recovery]);
  const currentCheckpointView = runtimeRef.current?.checkpoint.getSnapshot() ?? checkpointView;
  return {
    saveView,
    checkpointView: currentCheckpointView,
    recovery,
    initializationError,
    ready: Boolean(saveView && currentCheckpointView),
    setTitle,
    setMarkdown,
    flush: () => runtimeRef.current ? flushRuntime(runtimeRef.current) : Promise.resolve(false),
    retry: async () => {
      const runtime = runtimeRef.current;
      if (!runtime) return false;
      if (runtime.checkpoint.getSnapshot().status === "failed" && !(await runtime.checkpoint.retry())) return false;
      return runtime.save.retry();
    },
    keepMyChanges: (latest: NewspaperClippingDetail) =>
      runtimeRef.current?.save.keepMyChanges(documentFromDetail(latest)) ?? Promise.resolve(false),
    useSavedVersion,
    discardInvalidRecovery
  };
}
