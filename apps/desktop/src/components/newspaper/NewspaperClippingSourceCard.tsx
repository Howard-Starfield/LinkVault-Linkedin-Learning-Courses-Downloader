import { AlignCenter, AlignLeft, AlignRight, ExternalLink, ImageOff, LoaderCircle, RotateCcw, X, ZoomIn } from "lucide-react";
import { useState, type ReactNode } from "react";
import { Button } from "../primitives";
import type { NewspaperClippingDetail } from "./newspaper-api";
import { useClippingImagePreview } from "./useClippingImagePreview";

type ClippingImageAlignment = "left" | "wide" | "right";

const CLIPPING_IMAGE_ALIGNMENT_KEY = "linkvault.clippingImageAlignment.v1";

function initialAlignment(): ClippingImageAlignment {
  try {
    const saved = window.localStorage.getItem(CLIPPING_IMAGE_ALIGNMENT_KEY);
    if (saved === "left" || saved === "wide" || saved === "right") return saved;
  } catch {
    // Presentation preferences are best-effort and never note-storage authority.
  }
  return "left";
}

export function NewspaperClippingSourceCard({
  detail,
  onOpenSource,
  onRetryAsset,
  recovering
}: {
  detail: NewspaperClippingDetail;
  onOpenSource: (detail: NewspaperClippingDetail) => void;
  onRetryAsset: () => Promise<boolean>;
  recovering: boolean;
}) {
  const [imageFailed, setImageFailed] = useState(false);
  const [alignment, setAlignment] = useState<ClippingImageAlignment>(initialAlignment);
  const preview = useClippingImagePreview();
  const chooseAlignment = (next: ClippingImageAlignment) => {
    setAlignment(next);
    try {
      window.localStorage.setItem(CLIPPING_IMAGE_ALIGNMENT_KEY, next);
    } catch {
      // The visible choice still applies for this session.
    }
  };
  const retryAsset = async () => {
    if (await onRetryAsset()) setImageFailed(false);
  };
  const unavailable = detail.assetState !== "ready"
    || detail.storageStatus === "offline"
    || detail.storageStatus === "marker_mismatch"
    || imageFailed;
  const message = detail.storageStatus === "offline"
    ? "Snapshot storage is offline. Recover the snapshot folder in Settings; your note is still available."
    : detail.storageStatus === "marker_mismatch"
      ? "This snapshot folder no longer has the expected LinkedVault marker."
      : detail.assetState === "missing" || imageFailed
        ? "The clipping image could not be verified. The saved note has been preserved."
        : "The clipping image is temporarily unavailable.";
  const caption = (
    <figcaption>
      <div>
        <strong>{detail.editionName}</strong>
        <span aria-hidden="true">·</span>
        <span>{detail.publicationDate}</span>
        <span aria-hidden="true">·</span>
        <span>Page {detail.pageNumber}</span>
      </div>
      {detail.sourceAvailable ? (
        <Button aria-label="Open source newspaper page" onClick={() => onOpenSource(detail)} size="xs" variant="ghost">
          <ExternalLink aria-hidden="true" /> Open source
        </Button>
      ) : (
        <p className="clipping-source-card__source-unavailable">
          Original edition is no longer in the newspaper library.<br />
          Your clipping and note are still saved.
        </p>
      )}
    </figcaption>
  );

  return (
    <figure className="clipping-source-card" data-alignment={alignment} aria-label="Saved newspaper clipping source">
      {unavailable ? (
        <>
          <div className="clipping-source-card__unavailable" role="status">
            <ImageOff aria-hidden="true" />
            <span>{message}</span>
            {detail.assetState === "missing" || imageFailed ? (
              <Button disabled={recovering} onClick={() => void retryAsset()} size="xs" variant="outline">
                {recovering ? <LoaderCircle aria-hidden="true" className="animate-spin" /> : <RotateCcw aria-hidden="true" />}
                Retry image check
              </Button>
            ) : null}
          </div>
          {caption}
        </>
      ) : (
        <>
          <div className="clipping-source-card__media">
            <button
              aria-label="Zoom clipping image"
              className="clipping-source-card__image-button"
              onClick={preview.openPreview}
              ref={preview.triggerRef}
              title="Open full-size clipping"
              type="button"
            >
              <img
                alt={`Clipping from ${detail.editionName}, ${detail.publicationDate}, page ${detail.pageNumber}`}
                draggable={false}
                onError={() => {
                  setImageFailed(true);
                  if (preview.open) preview.closePreview();
                }}
                src={detail.imageUrl}
              />
              <span aria-hidden="true" className="clipping-source-card__zoom-cue"><ZoomIn /></span>
            </button>
            <div aria-label="Clipping image alignment" className="clipping-source-card__alignment" role="toolbar">
              <AlignmentButton active={alignment === "left"} label="Align clipping left" onClick={() => chooseAlignment("left")}><AlignLeft /></AlignmentButton>
              <AlignmentButton active={alignment === "wide"} label="Show clipping full width" onClick={() => chooseAlignment("wide")}><AlignCenter /></AlignmentButton>
              <AlignmentButton active={alignment === "right"} label="Align clipping right" onClick={() => chooseAlignment("right")}><AlignRight /></AlignmentButton>
            </div>
            {caption}
          </div>
          <dialog
            aria-label="Clipping image preview"
            className="clipping-source-card__lightbox"
            data-dragging={preview.dragging ? "true" : "false"}
            data-zoomed={preview.scale > 1 ? "true" : "false"}
            onCancel={(event) => {
              event.preventDefault();
              preview.closePreview();
            }}
            onClick={(event) => {
              if (event.target === event.currentTarget) preview.closePreview();
            }}
            onClose={preview.closePreview}
            ref={preview.dialogRef}
          >
            <button aria-label="Close image preview" onClick={preview.closePreview} type="button"><X aria-hidden="true" /></button>
            <div className="clipping-source-card__lightbox-viewport" ref={preview.viewportRef}>
              <img
                alt={`Full clipping from ${detail.editionName}, ${detail.publicationDate}, page ${detail.pageNumber}`}
                aria-label={preview.scale > 1 ? "Fit clipping preview" : "Zoom in clipping preview"}
                data-dragging={preview.dragging ? "true" : "false"}
                data-zoomed={preview.scale > 1 ? "true" : "false"}
                draggable={false}
                onKeyDown={preview.onImageKeyDown}
                onPointerCancel={preview.onImagePointerCancel}
                onPointerDown={preview.onImagePointerDown}
                onPointerMove={preview.onImagePointerMove}
                onPointerUp={preview.onImagePointerUp}
                ref={preview.imageRef}
                role="button"
                src={detail.imageUrl}
                style={preview.imageStyle}
                tabIndex={0}
              />
            </div>
            <span aria-hidden="true" className="clipping-source-card__lightbox-hint">
              {preview.scale > 1 ? "Drag to move · click to fit" : "Click to zoom"}
            </span>
          </dialog>
        </>
      )}
    </figure>
  );
}

function AlignmentButton({ active, children, label, onClick }: {
  active: boolean;
  children: ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button aria-label={label} aria-pressed={active} onClick={onClick} title={label} type="button">
      {children}
    </button>
  );
}
