import { useCallback, useEffect, useRef, useState } from "react";
import type { NewspaperReaderSourceTarget } from "./newspaper-navigation";

const SOURCE_HIGHLIGHT_DURATION_MS = 3_000;

export function useNewspaperSourceHighlight(target?: NewspaperReaderSourceTarget | null) {
  const timerRef = useRef<number | null>(null);
  const [visiblePageId, setVisiblePageId] = useState<string | null>(null);

  const clear = useCallback(() => {
    if (timerRef.current !== null) window.clearTimeout(timerRef.current);
    timerRef.current = null;
    setVisiblePageId(null);
  }, []);

  const show = useCallback((pageId: string) => {
    clear();
    if (!target || pageId !== target.pageId) return;
    setVisiblePageId(pageId);
    timerRef.current = window.setTimeout(clear, SOURCE_HIGHLIGHT_DURATION_MS);
  }, [clear, target]);

  useEffect(() => {
    clear();
    return clear;
  }, [clear, target?.generation]);

  return { clear, show, visiblePageId };
}
