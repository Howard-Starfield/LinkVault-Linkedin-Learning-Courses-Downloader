import { AlertTriangle, LoaderCircle, Trash2 } from "lucide-react";
import { Button, Dialog } from "../primitives";

export function NewspaperClippingDeleteDialog({
  deleting,
  error,
  open,
  onCancel,
  onConfirm
}: {
  deleting: boolean;
  error: string;
  open: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <Dialog
      description="This action cannot be undone."
      onOpenChange={(next) => { if (!next && !deleting) onCancel(); }}
      open={open}
      title="Delete this clipping?"
    >
      <div className="clipping-delete-dialog__copy">
        <AlertTriangle aria-hidden="true" />
        <p>
          This removes the saved image and its note from LinkedVault. The original
          newspaper page is not deleted.
        </p>
      </div>
      {error ? <p className="clipping-delete-dialog__error" role="alert">{error}</p> : null}
      <div className="clipping-delete-dialog__actions">
        <Button autoFocus disabled={deleting} onClick={onCancel} variant="outline">Cancel</Button>
        <Button disabled={deleting} onClick={onConfirm} variant="danger">
          {deleting ? <LoaderCircle aria-hidden="true" className="animate-spin" /> : <Trash2 aria-hidden="true" />}
          {deleting ? "Deleting…" : "Delete clipping"}
        </Button>
      </div>
    </Dialog>
  );
}
