import { useEffect } from "react";
import type { CoursePlayback, PlayerSession, PlaybackTick, VideoPlayback, VideoProgress } from "../../lib/linkedin/types";
import { linkedinSaveProgress } from "../../lib/linkedin/ipc";

const SAVE_DEBOUNCE_MS = 400;

type LinkedinPlayerProps = {
  playback: CoursePlayback;
  session: PlayerSession;
  onClose: () => void;
  onProgress: (video: VideoPlayback, progress: VideoProgress) => void;
};

export function LinkedinPlayer({ playback, session, onClose, onProgress }: LinkedinPlayerProps) {
  if (!session) return null;
  const video = playback.chapters.flatMap((chapter) => chapter.videos).find((item) => item.video === session.video);
  if (!video?.mediaUrl) return null;

  return (
    <PlayerOverlay
      playback={playback}
      video={video}
      onClose={onClose}
      onProgress={onProgress}
    />
  );
}

function PlayerOverlay({
  playback,
  video,
  onClose,
  onProgress
}: {
  playback: CoursePlayback;
  video: VideoPlayback;
  onClose: () => void;
  onProgress: LinkedinPlayerProps["onProgress"];
}) {
  useEffect(() => {
    let timer: number | null = null;
    let pending: PlaybackTick | null = null;

    function flush() {
      if (timer !== null) {
        window.clearTimeout(timer);
        timer = null;
      }
      const tick = pending;
      pending = null;
      if (!tick) return;
      void linkedinSaveProgress(tick).then((progress) => {
        onProgress(video, progress);
      });
    }

    function queueTick(positionMs: number, durationMs: number) {
      if (!Number.isFinite(positionMs) || !Number.isFinite(durationMs) || durationMs <= 0) {
        return;
      }
      pending = {
        course: playback.course,
        video: video.video,
        positionMs: Math.max(0, Math.floor(positionMs)),
        durationMs: Math.max(0, Math.floor(durationMs))
      };
      if (timer !== null) return;
      timer = window.setTimeout(() => {
        timer = null;
        flush();
      }, SAVE_DEBOUNCE_MS);
    }

    const node = document.getElementById("linkedin-player-video");
    const media = node instanceof HTMLVideoElement ? node : null;
    if (!media) {
      return () => undefined;
    }

    function onTimeUpdate(event: Event) {
      const target = event.currentTarget;
      if (!(target instanceof HTMLVideoElement)) return;
      queueTick(target.currentTime * 1000, target.duration * 1000);
    }

    function onPauseOrEnded(event: Event) {
      const target = event.currentTarget;
      if (target instanceof HTMLVideoElement) {
        queueTick(target.currentTime * 1000, target.duration * 1000);
      }
      flush();
    }

    media.addEventListener("timeupdate", onTimeUpdate);
    media.addEventListener("pause", onPauseOrEnded);
    media.addEventListener("ended", onPauseOrEnded);
    return () => {
      media.removeEventListener("timeupdate", onTimeUpdate);
      media.removeEventListener("pause", onPauseOrEnded);
      media.removeEventListener("ended", onPauseOrEnded);
      flush();
    };
  }, [onProgress, playback.course, video]);

  return (
    <div className="linkedin-player-overlay" role="dialog" aria-label={video.title}>
      <div className="linkedin-player-toolbar">
        <button type="button" className="linkedin-player-back" onClick={onClose}>
          Back
        </button>
        <h2>{video.title}</h2>
      </div>
      <video
        id="linkedin-player-video"
        className="linkedin-player-video"
        src={video.mediaUrl ?? undefined}
        controls
        autoPlay
      />
    </div>
  );
}
