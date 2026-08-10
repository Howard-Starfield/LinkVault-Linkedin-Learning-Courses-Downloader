import { lazy, Suspense, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { createPortal } from "react-dom";
import { AlertTriangle, Check, Clipboard, LoaderCircle, RotateCcw } from "lucide-react";
import { Button, Input } from "../primitives";
import {
  clippingErrorCode,
  getNewspaperClipping,
  updateNewspaperClipping,
  type NewspaperClippingDetail as ClippingDetail
} from "./newspaper-api";
import {
  ClippingNoteSaveController,
  type ClippingNoteDocument
} from "./clipping-note-save-controller";
import { NewspaperClippingSourceCard } from "./NewspaperClippingSourceCard";

const LazyClippingNoteEditor = lazy(() => import("./ClippingNoteEditor").then((module) => ({
  default: module.ClippingNoteEditor
})));

function documentFromDetail(detail: ClippingDetail): ClippingNoteDocument {
  return {
    documentId: detail.id,
    title: detail.title,
    markdown: detail.noteMarkdown,
    revision: detail.revision
  };
}

function validateDraft({ title, markdown }: { title: string; markdown: string }) {
  const trimmed = title.trim();
  if (!trimmed || [...trimmed].length > 200 || new TextEncoder().encode(trimmed).length > 800) {
    return "CLIPPING_INVALID_TITLE";
  }
  if (markdown.includes("\0")) return "CLIPPING_INVALID_MARKDOWN";
  if (new TextEncoder().encode(markdown).length > 2_097_152) return "CLIPPING_NOTE_TOO_LARGE";
  return null;
}

export function NewspaperClippingDetail({
  detail,
  onSaved,
  registerFlush
}: {
  detail: ClippingDetail;
  onSaved: (detail: ClippingDetail) => void;
  registerFlush: (flush: (() => Promise<boolean>) | null) => void;
}) {
  const controller = useMemo(() => new ClippingNoteSaveController(
    documentFromDetail(detail),
    async (request) => {
      const updated = await updateNewspaperClipping({
        clippingId: request.documentId,
        expectedRevision: request.expectedRevision,
        title: request.title,
        noteMarkdown: request.markdown
      });
      onSaved(updated);
      return documentFromDetail(updated);
    },
    clippingErrorCode,
    800,
    validateDraft
  ), [detail.id]);
  const view = useSyncExternalStore(controller.subscribe, controller.getSnapshot, controller.getSnapshot);
  const [latestConflict, setLatestConflict] = useState<ClippingDetail | null>(null);
  const [conflictLoading, setConflictLoading] = useState(false);
  const [editorIdentity, setEditorIdentity] = useState(0);
  const [copyFallback, setCopyFallback] = useState(false);

  useEffect(() => {
    registerFlush(() => controller.flush());
    return () => {
      registerFlush(null);
      controller.dispose();
    };
  }, [controller, registerFlush]);

  useEffect(() => {
    if (view.status !== "conflict" || latestConflict) return;
    let stale = false;
    setConflictLoading(true);
    void getNewspaperClipping(detail.id)
      .then((latest) => {
        if (!stale) setLatestConflict(latest);
      })
      .finally(() => {
        if (!stale) setConflictLoading(false);
      });
    return () => {
      stale = true;
    };
  }, [detail.id, latestConflict, view.status]);

  useEffect(() => {
    const flushOnBlur = () => void controller.flush();
    window.addEventListener("blur", flushOnBlur);
    return () => window.removeEventListener("blur", flushOnBlur);
  }, [controller]);

  const statusCopy = view.errorCode === "CLIPPING_INVALID_TITLE"
    ? "Title must be 1–200 characters."
    : view.errorCode === "CLIPPING_NOTE_TOO_LARGE"
      ? "Note exceeds the 2 MiB limit."
      : view.errorCode === "CLIPPING_INVALID_MARKDOWN"
        ? "Note contains an invalid null character."
        : view.status === "saving"
          ? "Saving…"
          : view.status === "dirty"
            ? "Unsaved changes"
            : view.status === "failed"
              ? "Save failed. Your draft is still here."
              : view.status === "conflict"
                ? "Changed in another window"
                : "Saved";
  const titleSlot = typeof document !== "undefined"
    ? document.getElementById("clipping-detail-title-slot")
    : null;

  return (
    <>
      {titleSlot
        ? createPortal(
          <label className="clipping-detail__title">
            <span className="sr-only">Title</span>
            <Input
              aria-label="Clipping note title"
              aria-invalid={view.errorCode === "CLIPPING_INVALID_TITLE" || undefined}
              onBlur={() => void controller.flush()}
              onChange={(event) => controller.setTitle(event.target.value)}
              onKeyDown={(event) => {
                if (event.key !== "Enter") return;
                event.preventDefault();
                document.querySelector<HTMLElement>('.clipping-note-editor__content')?.focus();
              }}
              placeholder="Untitled clipping"
              value={view.draftTitle}
            />
          </label>,
          titleSlot
        )
        : null}
      <article className="clipping-detail" aria-label="Clipping note detail">
        <NewspaperClippingSourceCard detail={detail} />
        <div className="clipping-detail__writing">
          <Suspense fallback={<div className="clipping-editor-loading"><LoaderCircle aria-hidden="true" className="animate-spin" /> Loading note editor…</div>}>
          <LazyClippingNoteEditor
            key={`${detail.id}-${editorIdentity}`}
            autoFocus
            documentId={`${detail.id}-${editorIdentity}`}
            footerContent={(
              <div className="clipping-save-status" data-status={view.status} role="status">
                {view.status === "saving" ? <LoaderCircle aria-hidden="true" className="animate-spin" /> : <Check aria-hidden="true" />}
                <span>{statusCopy}</span>
                {view.status === "failed" && !view.errorCode?.startsWith("CLIPPING_INVALID") && view.errorCode !== "CLIPPING_NOTE_TOO_LARGE" ? (
                  <Button size="xs" variant="outline" onClick={() => void controller.retry()}>
                    <RotateCcw aria-hidden="true" /> Retry
                  </Button>
                ) : null}
              </div>
            )}
            initialMarkdown={view.draftMarkdown}
            onBlur={() => void controller.flush()}
            onMarkdownChange={(markdown) => controller.setMarkdown(markdown)}
          />
          </Suspense>
        {view.status === "conflict" ? (
          <section className="clipping-conflict" role="alert">
            <div><AlertTriangle aria-hidden="true" /><strong>This note changed in another window.</strong></div>
            <p>Your local draft has not been overwritten. Choose which version to keep.</p>
            <div className="clipping-conflict__actions">
              <Button
                disabled={!latestConflict || conflictLoading}
                onClick={() => {
                  if (!latestConflict) return;
                  const latest = latestConflict;
                  setLatestConflict(null);
                  void controller.keepMyChanges(documentFromDetail(latest));
                }}
                variant="primary"
              >Keep my changes</Button>
              <Button
                disabled={!latestConflict || conflictLoading}
                onClick={() => {
                  if (!latestConflict) return;
                  controller.useSavedVersion(documentFromDetail(latestConflict));
                  onSaved(latestConflict);
                  setEditorIdentity((current) => current + 1);
                  setLatestConflict(null);
                }}
                variant="outline"
              >Use saved version</Button>
              <Button
                onClick={() => void navigator.clipboard.writeText(view.draftMarkdown).then(
                  () => setCopyFallback(false),
                  () => setCopyFallback(true)
                )}
                variant="outline"
              ><Clipboard aria-hidden="true" /> Copy my draft</Button>
            </div>
            {copyFallback ? <textarea aria-label="Copy clipping draft manually" readOnly value={view.draftMarkdown} /> : null}
          </section>
        ) : null}
        </div>
      </article>
    </>
  );
}
