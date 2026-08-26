import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import {
  canListenForClippingInstanceActivation,
  listenForClippingInstanceActivation
} from "./clipping-instance-activation";
import { getNewspaperClipping, isTauriRuntime, type NewspaperClippingDetail } from "./newspaper-api";

export type ClippingFlush = () => Promise<boolean>;

export function useNewspaperClippingSelection({
  pendingClippingId,
  pendingFocusEditor,
  pendingFocusSource,
  onDetailStateChange,
  onPendingConsumed,
  registerFlush
}: {
  pendingClippingId: string | null;
  pendingFocusEditor: boolean;
  pendingFocusSource: boolean;
  onDetailStateChange: (open: boolean) => void;
  onPendingConsumed: () => void;
  registerFlush: (flush: ClippingFlush | null) => void;
}) {
  const [selectedId, setSelectedId] = useState<string | null>(pendingClippingId);
  const [detail, setDetail] = useState<NewspaperClippingDetail | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [focusEditor, setFocusEditor] = useState(pendingFocusEditor);
  const [focusSource, setFocusSource] = useState(pendingFocusSource);
  const [detailIdentity, setDetailIdentity] = useState(0);
  const detailFlushRef = useRef<ClippingFlush | null>(null);
  const cachedOrderRef = useRef<string[]>([]);
  const selectedOrderRef = useRef<string[]>([]);
  const requestGenerationRef = useRef(0);

  const setRegisteredFlush = useCallback((flush: ClippingFlush | null) => {
    detailFlushRef.current = flush;
    registerFlush(flush);
  }, [registerFlush]);
  const recordOrderedIds = useCallback((ids: string[]) => {
    cachedOrderRef.current = ids;
  }, []);
  const loadDetail = useCallback(async (id: string, remount = false) => {
    const generation = ++requestGenerationRef.current;
    setLoading(true);
    setError("");
    try {
      const next = await getNewspaperClipping(id);
      if (generation === requestGenerationRef.current) {
        setDetail(next);
        if (remount) setDetailIdentity((current) => current + 1);
      }
    } catch (cause) {
      if (generation !== requestGenerationRef.current) return;
      setDetail(null);
      setError(String(cause));
    } finally {
      if (generation === requestGenerationRef.current) setLoading(false);
    }
  }, []);
  const select = useCallback(async (id: string, nextFocusEditor = false, nextFocusSource = false) => {
    if (id === selectedId) return;
    if (detailFlushRef.current && !(await detailFlushRef.current())) {
      toast.error("This note still has unsaved changes", {
        description: "Retry the save or resolve the conflict before opening another clipping."
      });
      return;
    }
    selectedOrderRef.current = [...cachedOrderRef.current];
    setFocusEditor(nextFocusEditor);
    setFocusSource(nextFocusSource);
    setSelectedId(id);
  }, [selectedId]);

  useEffect(() => onDetailStateChange(selectedId !== null), [onDetailStateChange, selectedId]);
  useEffect(() => () => onDetailStateChange(false), [onDetailStateChange]);
  useEffect(() => {
    if (!pendingClippingId) return;
    void select(pendingClippingId, pendingFocusEditor, pendingFocusSource).finally(onPendingConsumed);
  }, [onPendingConsumed, pendingClippingId, pendingFocusEditor, pendingFocusSource, select]);
  useEffect(() => {
    if (!selectedId) {
      requestGenerationRef.current += 1;
      setDetail(null);
      setLoading(false);
      setError("");
      return;
    }
    void loadDetail(selectedId);
  }, [loadDetail, selectedId]);
  useEffect(() => {
    if (!selectedId || !isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<{ clippingId?: string; reason?: string }>("newspaper://clipping-invalidated", (event) => {
      const { clippingId, reason } = event.payload ?? {};
      if (reason !== "source_changed" || (clippingId && clippingId !== selectedId)) return;
      void getNewspaperClipping(selectedId).then((next) => {
        if (!disposed) setDetail(next);
      }, () => undefined);
    }).then((cleanup) => { unlisten = cleanup; });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [selectedId]);
  useEffect(() => {
    if (!selectedId || !canListenForClippingInstanceActivation()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listenForClippingInstanceActivation(async () => {
      if (detailFlushRef.current && !(await detailFlushRef.current())) {
        toast.error("LinkedVault kept your current draft open", {
          description: "Resolve or retry the note save before refreshing this clipping."
        });
        return;
      }
      if (!disposed) await loadDetail(selectedId, true);
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [loadDetail, selectedId]);

  const handleDeleted = useCallback((clippingId: string) => {
    setRegisteredFlush(null);
    setDetail(null);
    const previousOrder = selectedOrderRef.current.filter((id) => id !== clippingId);
    const deletedIndex = Math.max(0, selectedOrderRef.current.indexOf(clippingId));
    const nextId = previousOrder[deletedIndex] ?? previousOrder[deletedIndex - 1] ?? null;
    selectedOrderRef.current = previousOrder;
    setFocusEditor(false);
    setFocusSource(false);
    setSelectedId(nextId);
  }, [setRegisteredFlush]);

  return {
    detail,
    detailIdentity,
    error,
    focusEditor,
    focusSource,
    handleDeleted,
    loading,
    recordOrderedIds,
    select,
    selectedId,
    setDetail,
    setRegisteredFlush
  };
}
