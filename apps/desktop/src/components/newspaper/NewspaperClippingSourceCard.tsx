import { ImageOff } from "lucide-react";
import { useState } from "react";
import type { NewspaperClippingDetail } from "./newspaper-api";

export function NewspaperClippingSourceCard({ detail }: { detail: NewspaperClippingDetail }) {
  const [imageFailed, setImageFailed] = useState(false);
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
        <strong>{detail.editionName}</strong>
        <span aria-hidden="true">·</span>
        <span>{detail.publicationDate}</span>
        <span aria-hidden="true">·</span>
        <span>Page {detail.pageNumber}</span>
      </figcaption>
    </figure>
  );
}
