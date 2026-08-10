import { LoaderCircle } from "lucide-react";
import type { NewspaperClippingDetail } from "./newspaper-api";
import { NewspaperClippingDetail as ClippingDetailView } from "./NewspaperClippingDetail";
import { NewspaperClippingList } from "./NewspaperClippingList";
import { useNewspaperClippingSelection, type ClippingFlush } from "./useNewspaperClippingSelection";

export type { ClippingFlush } from "./useNewspaperClippingSelection";

export function NewspaperClippings({
  pendingClippingId,
  pendingFocusEditor,
  pendingFocusSource,
  initialGalleryScrollTop,
  onDetailStateChange,
  onGallerySummaryChange,
  onGalleryScrollTopChange,
  onOpenLibrary,
  onOpenSource,
  onPendingConsumed,
  registerFlush
}: {
  pendingClippingId: string | null;
  pendingFocusEditor: boolean;
  pendingFocusSource: boolean;
  initialGalleryScrollTop: number;
  onDetailStateChange: (open: boolean) => void;
  onGallerySummaryChange: (summary: { total: number; loading: boolean } | null) => void;
  onGalleryScrollTopChange: (scrollTop: number) => void;
  onOpenLibrary: () => void;
  onOpenSource: (detail: NewspaperClippingDetail) => void;
  onPendingConsumed: () => void;
  registerFlush: (flush: ClippingFlush | null) => void;
}) {
  const selection = useNewspaperClippingSelection({
    pendingClippingId,
    pendingFocusEditor,
    pendingFocusSource,
    onDetailStateChange,
    onPendingConsumed,
    registerFlush
  });

  return (
    <>
      <NewspaperClippingList
        hidden={selection.selectedId !== null}
        initialScrollTop={initialGalleryScrollTop}
        onOpenLibrary={onOpenLibrary}
        onOrderedIdsChange={selection.recordOrderedIds}
        onScrollTopChange={onGalleryScrollTopChange}
        onSelect={(id) => void selection.select(id)}
        onSummaryChange={onGallerySummaryChange}
      />
      {selection.selectedId ? <section className="clipping-note-page" aria-label="Clipping note"><main className="clipping-note-page__body">
        {selection.loading ? <div className="clipping-detail-state"><LoaderCircle aria-hidden="true" className="animate-spin" /> Loading clipping…</div> : null}
        {!selection.loading && selection.error ? <div className="clipping-detail-state" role="alert">Could not load this clipping. {selection.error}</div> : null}
        {!selection.loading && selection.detail ? (
          <ClippingDetailView
            detail={selection.detail}
            focusEditor={selection.focusEditor}
            focusSource={selection.focusSource}
            key={selection.detail.id}
            onDeleted={selection.handleDeleted}
            onOpenSource={onOpenSource}
            onSaved={selection.setDetail}
            registerFlush={selection.setRegisteredFlush}
          />
        ) : null}
      </main></section> : null}
    </>
  );
}
