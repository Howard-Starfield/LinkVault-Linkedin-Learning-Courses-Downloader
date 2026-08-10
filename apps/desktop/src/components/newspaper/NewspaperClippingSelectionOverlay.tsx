import { useEffect, useRef } from "react";
import { Button } from "../primitives";
import type { EstimatedCropSize } from "./newspaper-clipping-geometry";
import type { NormalizedCropRect } from "./newspaper-api";

export function NewspaperClippingSelectionOverlay({
  rect,
  estimatedSize
}: {
  rect: NormalizedCropRect;
  estimatedSize: EstimatedCropSize | null;
}) {
  const style = {
    left: `${rect.x * 100}%`,
    top: `${rect.y * 100}%`,
    width: `${rect.width * 100}%`,
    height: `${rect.height * 100}%`
  };
  return (
    <div className="newspaper-clipping-overlay" data-testid="newspaper-clipping-selection">
      <div className="newspaper-clipping-selection" style={style} aria-hidden="true">
        {estimatedSize ? (
          <span className="newspaper-clipping-size">
            Approximately {estimatedSize.width} × {estimatedSize.height} px
          </span>
        ) : null}
      </div>
    </div>
  );
}

export function NewspaperClippingConfirmationControls({
  saving,
  waiting,
  saveDisabled,
  error,
  onSave,
  onRedraw,
  onCancel
}: {
  saving: boolean;
  waiting: boolean;
  saveDisabled: boolean;
  error?: string;
  onSave: () => void;
  onRedraw: () => void;
  onCancel: () => void;
}) {
  const groupRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (saving) return;
    const testId = saveDisabled ? "newspaper-clipping-redraw" : "newspaper-clipping-save";
    groupRef.current
      ?.querySelector<HTMLElement>(`[data-testid="${testId}"]`)
      ?.focus({ preventScroll: true });
  }, [saveDisabled, saving]);

  return (
    <div ref={groupRef} className="newspaper-clipping-confirm" data-testid="newspaper-clipping-confirm" role="group" aria-label="Confirm clipping">
      {error ? <p role="alert">{error}</p> : null}
      <div>
        <Button
          size="xs"
          variant="primary"
          disabled={saving || saveDisabled}
          loading={saving}
          loadingLabel={waiting ? "Waiting to save" : "Saving"}
          data-testid="newspaper-clipping-save"
          onClick={onSave}
        >
          Save clipping
        </Button>
        <Button
          size="xs"
          variant="secondary"
          disabled={saving}
          data-testid="newspaper-clipping-redraw"
          onClick={onRedraw}
        >
          Redraw
        </Button>
        <Button size="xs" variant="ghost" disabled={saving} data-testid="newspaper-clipping-cancel" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </div>
  );
}
