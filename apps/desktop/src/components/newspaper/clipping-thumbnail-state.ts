export type StableClippingThumbnail = {
  id: string;
  assetState: string;
  assetVersion: number;
  thumbnailReady: boolean;
  thumbnailUrl?: string | null;
  thumbnailVersion?: string | null;
};

/**
 * List refreshes carry canonical database state, while thumbnails are derived
 * application-cache state. Preserve a proven cache URL across note/title
 * invalidations only while the clipping ID and immutable asset version match.
 */
export function preserveStableClippingThumbnail<T extends StableClippingThumbnail>(
  incoming: T,
  previous: T | undefined
): T {
  if (
    incoming.assetState !== "ready"
    || !previous?.thumbnailReady
    || !previous.thumbnailUrl
    || previous.id !== incoming.id
    || previous.assetVersion !== incoming.assetVersion
  ) {
    return incoming;
  }
  return {
    ...incoming,
    thumbnailReady: true,
    thumbnailUrl: previous.thumbnailUrl,
    thumbnailVersion: previous.thumbnailVersion
  };
}
