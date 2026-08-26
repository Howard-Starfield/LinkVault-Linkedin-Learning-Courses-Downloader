import { lazy, Suspense, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { toast } from "sonner";
import { AlertTriangle, Check, Clipboard, LoaderCircle, MoreHorizontal, RotateCcw } from "lucide-react";
import { Button, Input } from "../primitives";
import {
  getNewspaperClipping,
  recoverNewspaperClippingAsset,
  type NewspaperClippingDetail as ClippingDetail
} from "./newspaper-api";
import { NewspaperClippingSourceCard } from "./NewspaperClippingSourceCard";
import { NewspaperClippingDeleteDialog } from "./NewspaperClippingDeleteDialog";
import { clippingSaveStatusCopy } from "./clipping-note-status";
import { useClippingNoteDurability } from "./useClippingNoteDurability";
import { useNewspaperClippingDelete } from "./useNewspaperClippingDelete";

const LazyClippingNoteEditor = lazy(() => import("./ClippingNoteEditor").then((module) => ({
  default: module.ClippingNoteEditor
})));

export function NewspaperClippingDetail({
  detail,
  focusEditor,
  focusSource,
  onDeleted,
  onOpenSource,
  onSaved,
  registerFlush
}: {
  detail: ClippingDetail;
  focusEditor: boolean;
  focusSource: boolean;
  onDeleted: (clippingId: string) => void;
  onOpenSource: (detail: ClippingDetail) => void;
  onSaved: (detail: ClippingDetail) => void;
  registerFlush: (flush: (() => Promise<boolean>) | null) => void;
}) {
  const durability = useClippingNoteDurability({ detail, onSaved, registerFlush });
  const view = durability.saveView;
  const [latestConflict, setLatestConflict] = useState<ClippingDetail | null>(null);
  const [conflictLoading, setConflictLoading] = useState(false);
  const [conflictLoadFailed, setConflictLoadFailed] = useState(false);
  const [conflictLoadAttempt, setConflictLoadAttempt] = useState(0);
  const [editorIdentity, setEditorIdentity] = useState(0);
  const [copyFallback, setCopyFallback] = useState(false);
  const [recoveringAsset, setRecoveringAsset] = useState(false);

  useEffect(() => {
    if (view?.status !== "conflict" || latestConflict) return;
    let stale = false;
    setConflictLoading(true);
    setConflictLoadFailed(false);
    void getNewspaperClipping(detail.id)
      .then((latest) => {
        if (stale) return;
        setConflictLoading(false);
        setLatestConflict(latest);
      }, () => {
        if (stale) return;
        setConflictLoading(false);
        setConflictLoadFailed(true);
      });
    return () => {
      stale = true;
    };
  }, [conflictLoadAttempt, detail.id, latestConflict, view?.status]);

  useEffect(() => {
    if (!focusSource) return;
    document.querySelector<HTMLButtonElement>("[aria-label='Open source newspaper page']")?.focus();
  }, [durability.ready, focusSource]);

  const effectiveError = view?.errorCode ?? durability.checkpointView?.errorCode;
  const statusCopy = clippingSaveStatusCopy(
    view?.status,
    effectiveError,
    durability.checkpointView?.status
  );

  const retryAsset = async () => {
    setRecoveringAsset(true);
    try {
      const recovered = await recoverNewspaperClippingAsset(detail.id);
      onSaved(recovered);
      toast.success("Clipping image verified");
      return true;
    } catch {
      toast.error("The clipping image could not be recovered", {
        description: "The note is still saved. LinkedVault did not search other folders or recrop the page."
      });
      return false;
    } finally {
      setRecoveringAsset(false);
    }
  };
  const deletion = useNewspaperClippingDelete({
    clippingId: detail.id,
    expectedRevision: view?.persistedRevision ?? detail.revision,
    flush: durability.flush,
    onDeleted,
    onSaved
  });
  const sourceHeader = (
    <NewspaperClippingSourceCard detail={detail} onOpenSource={onOpenSource}
      onRetryAsset={retryAsset} recovering={recoveringAsset} />
  );

  if (!durability.ready || !view) {
    const invalid = durability.recovery?.status === "invalid";
    return (
      <article className="clipping-detail" aria-label="Clipping note detail">
        {sourceHeader}
        <div className="clipping-detail-state" role={invalid || durability.initializationError ? "alert" : "status"}>
          {invalid ? (
            <>
              <AlertTriangle aria-hidden="true" />
              <span>Recovered changes are invalid. The saved note is untouched.</span>
              {durability.recovery?.identity ? (
                <Button size="sm" variant="outline" onClick={() => void durability.discardInvalidRecovery()}>
                  Discard recovered changes
                </Button>
              ) : null}
            </>
          ) : durability.initializationError ? (
            <span>Could not prepare note recovery. {durability.initializationError}</span>
          ) : (
            <><LoaderCircle aria-hidden="true" className="animate-spin" /> Preparing note recovery…</>
          )}
        </div>
      </article>
    );
  }

  const titleSlot = typeof document !== "undefined"
    ? document.getElementById("clipping-detail-title-slot")
    : null;

  return (
    <>
      {titleSlot ? createPortal(
        <div className="clipping-detail__title-row">
          <label className="clipping-detail__title">
            <span className="sr-only">Title</span>
            <Input
            aria-label="Clipping note title"
            aria-invalid={effectiveError === "CLIPPING_INVALID_TITLE" || undefined}
            onBlur={() => void durability.flush()}
            onChange={(event) => durability.setTitle(event.target.value)}
            onKeyDown={(event) => {
              if (event.key !== "Enter") return;
              event.preventDefault();
              document.querySelector<HTMLElement>('.clipping-note-editor__content')?.focus();
            }}
            placeholder="Untitled clipping"
            value={view.draftTitle}
            />
          </label>
          <Button
            aria-label="Delete clipping"
            onClick={() => void deletion.request()}
            size="xs"
            variant="ghost"
          ><MoreHorizontal aria-hidden="true" /></Button>
        </div>,
        titleSlot
      ) : null}
      <article className="clipping-detail" aria-label="Clipping note detail">
        <div className="clipping-detail__writing">
          {durability.recovery?.status === "matching" ? (
            <div className="clipping-recovery-notice" role="status">
              <RotateCcw aria-hidden="true" /> Recovered unsaved changes
            </div>
          ) : null}
          <Suspense fallback={<div className="clipping-editor-loading"><LoaderCircle aria-hidden="true" className="animate-spin" /> Loading note editor…</div>}>
            <LazyClippingNoteEditor
              key={`${detail.id}-${editorIdentity}`}
              autoFocus={focusEditor}
              documentId={`${detail.id}-${editorIdentity}`}
              headerContent={sourceHeader}
              footerContent={(
                <div className="clipping-save-status" data-status={view.status} role="status">
                  {view.status === "saving" ? <LoaderCircle aria-hidden="true" className="animate-spin" /> : <Check aria-hidden="true" />}
                  <span>{statusCopy}</span>
                  {view.status === "failed" && !effectiveError?.startsWith("CLIPPING_INVALID") && effectiveError !== "CLIPPING_NOTE_TOO_LARGE" ? (
                    <Button size="xs" variant="outline" onClick={() => void durability.retry()}>
                      <RotateCcw aria-hidden="true" /> Retry
                    </Button>
                  ) : null}
                </div>
              )}
              initialMarkdown={view.draftMarkdown}
              onBlur={() => void durability.flush()}
              onMarkdownChange={durability.setMarkdown}
            />
          </Suspense>
          {view.status === "conflict" ? (
            <section className="clipping-conflict" role="alert">
              <div><AlertTriangle aria-hidden="true" /><strong>This note changed in another window.</strong></div>
              <p>Your local draft has not been overwritten. Choose which version to keep.</p>
              {conflictLoadFailed ? (
                <div className="clipping-conflict__load-error">
                  <span>Could not load the saved version.</span>
                  <Button size="xs" variant="outline" onClick={() => setConflictLoadAttempt((current) => current + 1)}>
                    <RotateCcw aria-hidden="true" /> Retry
                  </Button>
                </div>
              ) : null}
              <div className="clipping-conflict__actions">
                <Button
                  disabled={!latestConflict || conflictLoading}
                  onClick={() => {
                    if (!latestConflict) return;
                    const latest = latestConflict;
                    setLatestConflict(null);
                    void durability.keepMyChanges(latest);
                  }}
                  variant="primary"
                >Keep my changes</Button>
                <Button
                  disabled={!latestConflict || conflictLoading}
                  onClick={() => {
                    if (!latestConflict) return;
                    void durability.useSavedVersion(latestConflict).then((used) => {
                      if (!used) return;
                      setEditorIdentity((current) => current + 1);
                      setLatestConflict(null);
                    });
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
      <NewspaperClippingDeleteDialog
        deleting={deletion.deleting}
        error={deletion.error}
        onCancel={deletion.cancel}
        onConfirm={() => void deletion.confirm()}
        open={deletion.open}
      />
    </>
  );
}
