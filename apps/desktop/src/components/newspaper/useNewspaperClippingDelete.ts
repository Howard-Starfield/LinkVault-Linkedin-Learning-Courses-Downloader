import { useCallback, useState } from "react";
import { toast } from "sonner";
import {
  clippingErrorCode,
  deleteNewspaperClipping,
  getNewspaperClipping,
  type NewspaperClippingDetail
} from "./newspaper-api";

export function useNewspaperClippingDelete({
  clippingId,
  expectedRevision,
  flush,
  onDeleted,
  onSaved
}: {
  clippingId: string;
  expectedRevision: number;
  flush: () => Promise<boolean>;
  onDeleted: (clippingId: string) => void;
  onSaved: (detail: NewspaperClippingDetail) => void;
}) {
  const [open, setOpen] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState("");

  const request = useCallback(async () => {
    if (!(await flush())) return;
    setError("");
    setOpen(true);
  }, [flush]);

  const confirm = useCallback(async () => {
    setDeleting(true);
    setError("");
    try {
      await deleteNewspaperClipping({ clippingId, expectedRevision });
      setOpen(false);
      onDeleted(clippingId);
    } catch (cause) {
      const code = clippingErrorCode(cause);
      if (code === "CLIPPING_DELETE_REVISION_CONFLICT" || code === "CLIPPING_REVISION_CONFLICT") {
        setOpen(false);
        toast.warning("This clipping changed after the dialog opened", {
          description: "Review the latest note, then choose Delete again."
        });
        void getNewspaperClipping(clippingId).then(onSaved);
      } else {
        setError("Clipping could not be deleted. Your image and note are still here.");
      }
    } finally {
      setDeleting(false);
    }
  }, [clippingId, expectedRevision, onDeleted, onSaved]);

  return {
    cancel: () => setOpen(false),
    confirm,
    deleting,
    error,
    open,
    request
  };
}
