import { ExternalLink, ImageOff, LoaderCircle, RotateCcw } from "lucide-react";
import { useState } from "react";
import { Button } from "../primitives";
import type { NewspaperClippingDetail } from "./newspaper-api";

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
  const retryAsset = async () => {
    if (await onRetryAsset()) setImageFailed(false);
  };
  const unavailable = detail.assetState !== "ready"
    || detail.storageStatus === "offline"
    || detail.storageStatus === "marker_mismatch"
    || imageFailed;
  const message = detail.storageStatus === "offline"
    ? "Snapshot storage is offline. Reconnect it in Settings; your note is still available."
    : detail.storageStatus === "marker_mismatch"
      ? "This snapshot folder no longer has the expected LinkVault marker."
      : detail.assetState === "missing" || imageFailed
        ? "The clipping image could not be verified. The saved note has been preserved."
        : "The clipping image is temporarily unavailable.";

  return (
    <figure className="clipping-source-card" aria-label="Saved newspaper clipping source">
      {unavailable ? (
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
      ) : (
        <img
          alt={`Clipping from ${detail.editionName}, ${detail.publicationDate}, page ${detail.pageNumber}`}
          draggable={false}
          onError={() => setImageFailed(true)}
          src={detail.imageUrl}
        />
      )}
      <figcaption>
        <div>
          <strong>{detail.editionName}</strong>
          <span aria-hidden="true">·</span>
          <span>{detail.publicationDate}</span>
          <span aria-hidden="true">·</span>
          <span>Page {detail.pageNumber}</span>
        </div>
        {detail.sourceAvailable ? (
          <Button
            aria-label="Open source newspaper page"
            onClick={() => onOpenSource(detail)}
            size="xs"
            variant="ghost"
          >
            <ExternalLink aria-hidden="true" /> Open source
          </Button>
        ) : (
          <p className="clipping-source-card__source-unavailable">
            Original edition is no longer in the newspaper library.<br />
            Your clipping and note are still saved.
          </p>
        )}
      </figcaption>
    </figure>
  );
}
