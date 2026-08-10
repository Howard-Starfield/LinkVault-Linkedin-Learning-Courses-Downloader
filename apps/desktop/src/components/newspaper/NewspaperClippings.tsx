import { FileText, LoaderCircle } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { getNewspaperClipping, type NewspaperClippingDetail } from "./newspaper-api";
import { NewspaperClippingDetail as ClippingDetailView } from "./NewspaperClippingDetail";
import { NewspaperClippingList } from "./NewspaperClippingList";

export type ClippingFlush = () => Promise<boolean>;

export function NewspaperClippings({
  pendingClippingId,
  onPendingConsumed,
  registerFlush
}: {
  pendingClippingId: string | null;
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
    if (!pendingClippingId) return;
    void select(pendingClippingId).finally(onPendingConsumed);
  }, [onPendingConsumed, pendingClippingId, select]);

  useEffect(() => {
    if (!selectedId) {
      setDetail(null);
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

  return (
    <section className="clippings-workspace" aria-label="Newspaper clippings">
      <header className="clippings-workspace__header">
        <div>
          <span className="clippings-workspace__eyebrow">World Journal · local evidence</span>
          <h2>Clippings</h2>
          <p>Saved source images and searchable notes stay together, even when an edition is removed.</p>
        </div>
      </header>
      <div className="clippings-workspace__body">
        <NewspaperClippingList selectedId={selectedId} onSelect={(id) => void select(id)} />
        <main className="clippings-workspace__detail">
          {loading ? <div className="clipping-detail-state"><LoaderCircle aria-hidden="true" className="animate-spin" /> Loading clipping…</div> : null}
          {!loading && error ? <div className="clipping-detail-state" role="alert">Could not load this clipping. {error}</div> : null}
          {!loading && !error && !detail ? (
            <div className="clipping-detail-state"><FileText aria-hidden="true" /> Select a clipping to open its note.</div>
          ) : null}
          {!loading && detail ? (
            <ClippingDetailView
              detail={detail}
              key={detail.id}
              onSaved={setDetail}
              registerFlush={setRegisteredFlush}
            />
          ) : null}
        </main>
      </div>
    </section>
  );
}
