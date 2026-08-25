export type DetectedYouTubeKind = "video" | "playlist" | "ambiguous";

export interface DetectedYouTubeLink {
  raw: string;
  canonicalUrl: string;
  kind: DetectedYouTubeKind;
  videoId: string | null;
  playlistId: string | null;
  complete: boolean;
}

const YOUTUBE_TOKEN_PATTERN =
  /(?:https?:\/\/)?(?:www\.|m\.)?(?:youtube\.com\/[^\s<>"'[\]]+|youtu\.be\/[^\s<>"'[\]]+)/gi;

const COMPLETE_VIDEO_ID = /^[\w-]{11}$/;
const COMPLETE_PLAYLIST_ID = /^[\w-]{2,}$/;

function stripWrappingPunctuation(value: string): string {
  return value.replace(/^[\s<(["']+/g, "").replace(/[\s>)\]"'.,;:!?]+$/g, "");
}

function normalizeHostname(hostname: string): string {
  return hostname.toLowerCase().replace(/^www\./, "");
}

function isYouTubeHost(hostname: string): boolean {
  const host = normalizeHostname(hostname);
  return host === "youtube.com" || host === "m.youtube.com" || host === "youtu.be";
}

function isCompleteVideoId(videoId: string | null): boolean {
  return videoId !== null && COMPLETE_VIDEO_ID.test(videoId);
}

function isCompletePlaylistId(playlistId: string | null): boolean {
  return playlistId !== null && COMPLETE_PLAYLIST_ID.test(playlistId);
}

export function parseYouTubeLink(raw: string): DetectedYouTubeLink | null {
  const trimmed = stripWrappingPunctuation(raw.trim());
  if (!trimmed) return null;
  const withProtocol = /^https?:\/\//i.test(trimmed) ? trimmed : `https://${trimmed}`;
  let url: URL;
  try {
    url = new URL(withProtocol);
  } catch {
    return null;
  }
  if (!isYouTubeHost(url.hostname)) return null;

  const host = normalizeHostname(url.hostname);
  const path = url.pathname.toLowerCase();
  const pathParts = url.pathname.split("/").filter(Boolean);
  const lastPathPart = pathParts.length > 0 ? pathParts[pathParts.length - 1] ?? "" : "";
  const playlistId = url.searchParams.get("list");
  const queryVideoId = url.searchParams.get("v");
  const shortOrLiveId = path.startsWith("/shorts/") || path.startsWith("/live/") || path.startsWith("/embed/") || path.startsWith("/v/")
    ? pathParts[1] ?? null
    : null;
  const youtuBeId = host === "youtu.be" ? pathParts[0] ?? null : null;
  const pathVideoId = lastPathPart !== "watch" && lastPathPart !== "playlist" && lastPathPart !== "shorts" && lastPathPart !== "live" && lastPathPart !== "embed"
    ? lastPathPart
    : null;
  const videoId = youtuBeId || queryVideoId || shortOrLiveId || pathVideoId || null;
  const hasVideo = videoId !== null && videoId.length > 0;
  const hasPlaylist = playlistId !== null && playlistId.length > 0;
  if (!hasVideo && !hasPlaylist) return null;

  const kind: DetectedYouTubeKind = hasVideo && hasPlaylist
    ? "ambiguous"
    : hasPlaylist && !hasVideo
      ? "playlist"
      : "video";
  const canonicalUrl = kind === "playlist"
    ? `https://www.youtube.com/playlist?list=${encodeURIComponent(playlistId ?? "")}`
    : kind === "ambiguous"
      ? `https://www.youtube.com/watch?v=${encodeURIComponent(videoId ?? "")}&list=${encodeURIComponent(playlistId ?? "")}`
      : host === "youtu.be" || path.startsWith("/shorts/")
        ? `https://www.youtube.com/watch?v=${encodeURIComponent(videoId ?? "")}`
        : `https://www.youtube.com/watch?v=${encodeURIComponent(videoId ?? "")}`;

  return {
    raw: trimmed,
    canonicalUrl,
    kind,
    videoId: videoId || null,
    playlistId: playlistId || null,
    complete: isCompleteVideoId(videoId) || (!hasVideo && isCompletePlaylistId(playlistId)) || (kind === "ambiguous" && isCompleteVideoId(videoId))
  };
}

export function isAmbiguousWatchPlaylist(raw: string): boolean {
  return parseYouTubeLink(raw)?.kind === "ambiguous";
}

export function detectYouTubeLinks(text: string): DetectedYouTubeLink[] {
  const seen = new Set<string>();
  const links: DetectedYouTubeLink[] = [];

  const push = (candidate: string): void => {
    const parsed = parseYouTubeLink(candidate);
    if (!parsed || seen.has(parsed.canonicalUrl)) return;
    seen.add(parsed.canonicalUrl);
    links.push(parsed);
  };

  const matches = text.matchAll(YOUTUBE_TOKEN_PATTERN);
  for (const match of matches) {
    push(match[0] ?? "");
  }

  if (links.length === 0) {
    push(text);
  }

  return links;
}

export function firstCompleteYouTubeLink(text: string): DetectedYouTubeLink | null {
  return detectYouTubeLinks(text).find((link) => link.complete) ?? null;
}

export function detectedKindLabel(kind: DetectedYouTubeKind): string {
  if (kind === "playlist") return "Playlist";
  if (kind === "ambiguous") return "Video + playlist";
  return "Video";
}
