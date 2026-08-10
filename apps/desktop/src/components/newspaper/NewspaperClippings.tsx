import { LoaderCircle } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { getNewspaperClipping, type NewspaperClippingDetail } from "./newspaper-api";
import { NewspaperClippingDetail as ClippingDetailView } from "./NewspaperClippingDetail";
import { NewspaperClippingList } from "./NewspaperClippingList";

export type ClippingFlush = () => Promise<boolean>;

export function NewspaperClippings({
  pendingClippingId,
  onDetailStateChange,
  onGallerySummaryChange,
  onOpenLibrary,
  onPendingConsumed,
  registerFlush
}: {
  pendingClippingId: string | null;
  onDetailStateChange: (open: boolean) => void;
  onGallerySummaryChange: (summary: { total: number; loading: boolean } | null) => void;
  onOpenLibrary: () => void;
  onPendingConsumed: () => void;
  registerFlush: (flush: ClippingFlush | null) => void;
}) {
  const [selectedId, setSelectedId] = useState<string | null>(pendingClippingId);
  const [detail, setDetail] = useState<NewspaperClippingDetail | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const detailFlushRef = useRef<ClippingFlush | null>(null);
  const requestGenerationRef = useRef(0);

  const setRegisteredFlush = useCallback((flush: ClippingFlush | null) => {
    detailFlushRef.current = flush;
    registerFlush(flush);
  }, [registerFlush]);

  const select = useCallback(async (id: string) => {
    if (id === selectedId) return;
    if (detailFlushRef.current && !(await detailFlushRef.current())) {
      toast.error("This note still has unsaved changes", {
        description: "Retry the save or resolve the conflict before opening another clipping."
      });
      return;
    }
    setSelectedId(id);
  }, [selectedId]);

  useEffect(() => {
    onDetailStateChange(selectedId !== null);
  }, [onDetailStateChange, selectedId]);

  useEffect(() => () => onDetailStateChange(false), [onDetailStateChange]);

  useEffect(() => {
    if (!pendingClippingId) return;
    void select(pendingClippingId).finally(onPendingConsumed);
  }, [onPendingConsumed, pendingClippingId, select]);

  useEffect(() => {
    if (!selectedId) {
      requestGenerationRef.current += 1;
      setDetail(null);
      setLoading(false);
      setError("");
      return;
    }
    const generation = requestGenerationRef.current + 1;
    requestGenerationRef.current = generation;
    setLoading(true);
    setError("");
    void getNewspaperClipping(selectedId)
      .then((next) => {
        if (generation === requestGenerationRef.current) setDetail(next);
      })
      .catch((cause) => {
        if (generation === requestGenerationRef.current) {
          setDetail(null);
          setError(String(cause));
        }
      })
      .finally(() => {
        if (generation === requestGenerationRef.current) setLoading(false);
      });
  }, [selectedId]);

  if (!selectedId) {
    return (
      <NewspaperClippingList
        onOpenLibrary={onOpenLibrary}
        onSelect={(id) => void select(id)}
        onSummaryChange={onGallerySummaryChange}
      />
    );
  }

  return (
    <section className="clipping-note-page" aria-label="Clipping note">
      <main className="clipping-note-page__body">
        {loading ? <div className="clipping-detail-state"><LoaderCircle aria-hidden="true" className="animate-spin" /> Loading clipping…</div> : null}
        {!loading && error ? <div className="clipping-detail-state" role="alert">Could not load this clipping. {error}</div> : null}
        {!loading && detail ? (
          <ClippingDetailView detail={detail} key={detail.id} onSaved={setDetail} registerFlush={setRegisteredFlush} />
        ) : null}
      </main>
    </section>
  );
}
