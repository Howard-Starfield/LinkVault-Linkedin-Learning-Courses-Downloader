import { useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronRight, FolderOpen, LoaderCircle, Play } from "lucide-react";
import { Button } from "../primitives";
import { formatYouTubeInvokeError, listYouTubeRunVideos } from "../../lib/youtube/ipc";
import { describeVideoProgress, playableVideos } from "../../lib/youtube/playback";
import type { YouTubeHistoryEntry, YouTubePlayerSession, YouTubeRunVideo } from "../../lib/youtube/types";

type YouTubeHistoryRowProps = {
  entry: YouTubeHistoryEntry;
  meta: string;
  /** Changes when the player closes so a loaded list picks up new resume positions. */
  refreshKey: number;
  onOpenFolder: (entry: YouTubeHistoryEntry) => void | Promise<void>;
  onPlay: (session: YouTubePlayerSession) => void;
};

/**
 * One history run. A single-video run plays directly; a multi-video run
 * expands into one row per video. The video list (and whether each file is
 * still on disk) is loaded on demand, never for the whole history at once.
 */
export function YouTubeHistoryRow({ entry, meta, refreshKey, onOpenFolder, onPlay }: YouTubeHistoryRowProps) {
  const [videos, setVideos] = useState<YouTubeRunVideo[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const loadedRef = useRef(false);
  const single = entry.videoCount === 1;
  const multiple = entry.videoCount > 1;

  async function loadVideos(): Promise<YouTubeRunVideo[] | null> {
    setLoading(true);
    setError(null);
    try {
      const list = await listYouTubeRunVideos({ runId: entry.runId });
      loadedRef.current = true;
      setVideos(list);
      return list;
    } catch (failure) {
      setError(formatYouTubeInvokeError(failure));
      return null;
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    if (refreshKey > 0 && loadedRef.current) {
      void loadVideos();
    }
  }, [refreshKey]);

  function start(list: YouTubeRunVideo[], occurrenceId: string) {
    onPlay({ runId: entry.runId, title: entry.title, videos: list, startOccurrenceId: occurrenceId });
  }

  async function playSingle() {
    const list = videos ?? (await loadVideos());
    if (!list) return;
    const first = playableVideos(list)[0];
    if (first) {
      start(list, first.occurrenceId);
    } else {
      setError(list[0]?.unavailableReason ?? "This video's files were not found.");
    }
  }

  async function toggleExpanded() {
    const next = !expanded;
    setExpanded(next);
    if (next && videos === null) {
      await loadVideos();
    }
  }

  const listId = `youtube-history-videos-${entry.runId}`;

  return (
    <li className="download-history-row youtube-history-row" data-expanded={expanded ? "true" : undefined}>
      <div className="download-history-copy">
        <strong>{entry.title}</strong>
        <span>{meta}</span>
        {entry.errorMessage ? <span className="youtube-result-warning">{entry.errorMessage}</span> : null}
        {single && error ? (
          <span className="youtube-result-warning" role="status">
            {error}
          </span>
        ) : null}
      </div>
      <div className="download-history-overlay">
        {single ? (
          <Button
            type="button"
            size="xs"
            variant="ghost"
            className="youtube-play-action"
            disabled={loading}
            onClick={() => void playSingle()}
            aria-label={`Play ${entry.title}`}
          >
            {loading ? <LoaderCircle aria-hidden="true" className="animate-spin" /> : <Play aria-hidden="true" />}
            Play
          </Button>
        ) : null}
        {multiple ? (
          <Button
            type="button"
            size="xs"
            variant="ghost"
            className="youtube-play-action"
            aria-expanded={expanded}
            aria-controls={listId}
            onClick={() => void toggleExpanded()}
            aria-label={`${expanded ? "Hide" : "Show"} ${entry.videoCount} videos in ${entry.title}`}
          >
            {expanded ? <ChevronDown aria-hidden="true" /> : <ChevronRight aria-hidden="true" />}
            Videos
          </Button>
        ) : null}
        <Button
          type="button"
          size="xs"
          variant="ghost"
          className="download-history-file-action youtube-file-action"
          onClick={() => void onOpenFolder(entry)}
          aria-label={`Open folder for ${entry.title}`}
        >
          <FolderOpen aria-hidden="true" />
          Open Folder
        </Button>
      </div>
      {multiple && expanded ? (
        <div id={listId} className="youtube-history-videos">
          {loading && videos === null ? (
            <p className="youtube-history-videos-note" role="status">
              Loading videos…
            </p>
          ) : null}
          {error ? (
            <p className="youtube-history-videos-note youtube-result-warning" role="alert">
              {error}
            </p>
          ) : null}
          {videos !== null && videos.length === 0 && !error ? (
            <p className="youtube-history-videos-note" role="status">
              No videos were recorded for this download.
            </p>
          ) : null}
          {videos !== null && videos.length > 0 ? (
            <ol className="youtube-history-video-list" aria-label={`Videos in ${entry.title}`}>
              {videos.map((video) => {
                const status = video.available ? describeVideoProgress(video) : video.unavailableReason;
                return (
                  <li
                    key={video.occurrenceId}
                    className="youtube-history-video"
                    data-available={video.available ? "true" : "false"}
                  >
                    <span className="youtube-history-video-ordinal" aria-hidden="true">
                      {video.ordinal}
                    </span>
                    <span className="youtube-history-video-copy">
                      <strong>{video.title}</strong>
                      {status ? (
                        <span className={video.available ? undefined : "youtube-result-warning"}>{status}</span>
                      ) : null}
                    </span>
                    <Button
                      type="button"
                      size="xs"
                      variant="ghost"
                      className="youtube-play-action"
                      disabled={!video.available}
                      title={video.unavailableReason ?? undefined}
                      onClick={() => start(videos, video.occurrenceId)}
                      aria-label={`Play ${video.title}`}
                    >
                      <Play aria-hidden="true" />
                      Play
                    </Button>
                  </li>
                );
              })}
            </ol>
          ) : null}
        </div>
      ) : null}
    </li>
  );
}
