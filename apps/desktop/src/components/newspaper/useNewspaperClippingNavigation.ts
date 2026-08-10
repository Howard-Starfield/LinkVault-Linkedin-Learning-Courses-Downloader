import { useCallback, useRef, useState } from "react";
import { toast } from "sonner";
import type { NewspaperClippingDetail } from "./newspaper-api";
import { readerTargetFromClipping, type NewspaperReaderSourceTarget } from "./newspaper-navigation";

type ClippingDestination = "newspaper-clippings" | "newspaper-library";
type RequestNavigation = (
  destination: ClippingDestination,
  options?: { preserveClippingContext?: boolean }
) => Promise<boolean>;

export function useNewspaperClippingNavigation({
  requestNavigation,
  setDetailOpen,
  setNewspaperExpanded
}: {
  requestNavigation: RequestNavigation;
  setDetailOpen: (open: boolean) => void;
  setNewspaperExpanded: (expanded: boolean) => void;
}) {
  const [pendingClippingId, setPendingClippingId] = useState<string | null>(null);
  const [pendingFocusEditor, setPendingFocusEditor] = useState(false);
  const [pendingFocusSource, setPendingFocusSource] = useState(false);
  const [pendingReaderTarget, setPendingReaderTarget] = useState<NewspaperReaderSourceTarget | null>(null);
  const [clippingsViewKey, setClippingsViewKey] = useState(0);
  const readerGenerationRef = useRef(0);

  const openClipping = useCallback(async (id: string, focusEditor = false, focusSource = false) => {
    if (!(await requestNavigation("newspaper-clippings"))) return;
    setPendingClippingId(id);
    setPendingFocusEditor(focusEditor);
    setPendingFocusSource(focusSource);
    setDetailOpen(true);
    setNewspaperExpanded(true);
  }, [requestNavigation, setDetailOpen, setNewspaperExpanded]);

  const openGallery = useCallback(async () => {
    if (!(await requestNavigation("newspaper-clippings"))) return;
    setPendingClippingId(null);
    setDetailOpen(false);
    setClippingsViewKey((current) => current + 1);
  }, [requestNavigation, setDetailOpen]);

  const openSource = useCallback(async (detail: NewspaperClippingDetail) => {
    const generation = readerGenerationRef.current + 1;
    const target = readerTargetFromClipping(detail, generation);
    if (!target) {
      toast.info("Original edition is no longer in the newspaper library", {
        description: "Your clipping and note are still saved."
      });
      return;
    }
    if (!(await requestNavigation("newspaper-library", { preserveClippingContext: true }))) return;
    readerGenerationRef.current = generation;
    setPendingReaderTarget(target);
    setNewspaperExpanded(true);
  }, [requestNavigation, setNewspaperExpanded]);

  const consumePendingClipping = useCallback(() => {
    setPendingClippingId(null);
    setPendingFocusEditor(false);
    setPendingFocusSource(false);
  }, []);
  const consumeReaderTarget = useCallback((generation: number) => {
    setPendingReaderTarget((current) => current?.generation === generation ? null : current);
  }, []);

  return {
    clippingsViewKey,
    consumePendingClipping,
    consumeReaderTarget,
    openClipping,
    openGallery,
    openSource,
    pendingClippingId,
    pendingFocusEditor,
    pendingFocusSource,
    pendingReaderTarget
  };
}
