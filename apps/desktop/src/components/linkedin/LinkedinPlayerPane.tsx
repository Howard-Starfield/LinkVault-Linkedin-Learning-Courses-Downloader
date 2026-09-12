import { useEffect, useRef, useState } from "react";
import { Pause, Play, Volume2, VolumeX } from "lucide-react";
import type { CoursePlayback, PlayerSession, PlaybackTick, VideoPlayback, VideoProgress } from "../../lib/linkedin/types";
import { resumePlaybackMs } from "../../lib/linkedin/playback";
import { linkedinSaveProgress } from "../../lib/linkedin/ipc";

const SAVE_DEBOUNCE_MS = 400;
const CHROME_HIDE_MS = 2000;
const CLOCK_UI_MS = 250;

type LinkedinPlayerPaneProps = {
  playback: CoursePlayback;
  session: PlayerSession;
  onClose: () => void;
  onProgress: (video: VideoPlayback, progress: VideoProgress) => void;
  onEnded: () => void;
};

export function LinkedinPlayerPane({
  playback,
  session,
  onClose,
  onProgress,
  onEnded
}: LinkedinPlayerPaneProps) {
  if (!session) return null;
  const video = playback.chapters.flatMap((chapter) => chapter.videos).find((item) => item.video === session.video);
  if (!video?.mediaUrl) return null;

  return (
    <PlayerPane
      key={video.video}
      playback={playback}
      video={video}
      onClose={onClose}
      onProgress={onProgress}
      onEnded={onEnded}
    />
  );
}

function PlayerPane({
  playback,
  video,
  onClose,
  onProgress,
  onEnded
}: {
  playback: CoursePlayback;
  video: VideoPlayback;
  onClose: () => void;
  onProgress: LinkedinPlayerPaneProps["onProgress"];
  onEnded: () => void;
}) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const onProgressRef = useRef(onProgress);
  const onEndedRef = useRef(onEnded);
  const resumeMsRef = useRef(resumePlaybackMs(video.progress));
  const hideTimerRef = useRef<number | null>(null);
  const [chromeVisible, setChromeVisible] = useState(true);
  const [paused, setPaused] = useState(false);
  const [muted, setMuted] = useState(false);
  const [positionSeconds, setPositionSeconds] = useState(resumePlaybackMs(video.progress) / 1000);
  const [durationSeconds, setDurationSeconds] = useState(
    video.progress.durationMs > 0 ? video.progress.durationMs / 1000 : 0
  );
  onProgressRef.current = onProgress;
  onEndedRef.current = onEnded;

  function showChrome() {
    setChromeVisible(true);
    if (hideTimerRef.current !== null) {
      window.clearTimeout(hideTimerRef.current);
    }
    hideTimerRef.current = window.setTimeout(() => {
      hideTimerRef.current = null;
      setChromeVisible(false);
    }, CHROME_HIDE_MS);
  }

  function hideChromeNow() {
    if (hideTimerRef.current !== null) {
      window.clearTimeout(hideTimerRef.current);
      hideTimerRef.current = null;
    }
    setChromeVisible(false);
  }

  useEffect(() => {
    showChrome();
    return () => {
      if (hideTimerRef.current !== null) {
        window.clearTimeout(hideTimerRef.current);
      }
    };
  }, [video.video]);

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
        onProgressRef.current(video, progress);
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

    const media = videoRef.current;
    if (!media) {
      return () => undefined;
    }

    let primed = false;
    let lastClockMs = 0;

    function applyResumeAndPlay(target: HTMLVideoElement) {
      if (primed) return;
      if (!Number.isFinite(target.duration) || target.duration <= 0) return;
      primed = true;
      const resumeSeconds = resumeMsRef.current / 1000;
      if (resumeSeconds > 0 && resumeSeconds < target.duration) {
        target.currentTime = resumeSeconds;
      } else {
        target.currentTime = 0;
      }
      syncClock(target, true);
      void target.play().catch(() => {
        setPaused(true);
      });
    }

    function syncClock(target: HTMLVideoElement, force = false) {
      const now = performance.now();
      if (!force && now - lastClockMs < CLOCK_UI_MS) {
        return;
      }
      lastClockMs = now;
      if (Number.isFinite(target.currentTime)) {
        setPositionSeconds(target.currentTime);
      }
      if (Number.isFinite(target.duration) && target.duration > 0) {
        setDurationSeconds(target.duration);
      }
      setPaused(target.paused);
      setMuted(target.muted);
    }

    function onTimeUpdate(event: Event) {
      const target = event.currentTarget;
      if (!(target instanceof HTMLVideoElement) || !primed) return;
      syncClock(target);
      queueTick(target.currentTime * 1000, target.duration * 1000);
    }

    function onPauseOrEnded(event: Event) {
      const target = event.currentTarget;
      if (target instanceof HTMLVideoElement && primed) {
        syncClock(target, true);
        queueTick(target.currentTime * 1000, target.duration * 1000);
      }
      if (primed) flush();
    }

    function onLoadedMetadata(event: Event) {
      const target = event.currentTarget;
      if (!(target instanceof HTMLVideoElement)) return;
      applyResumeAndPlay(target);
    }

    function onEndedMedia(event: Event) {
      if (!primed) return;
      onPauseOrEnded(event);
      onEndedRef.current();
    }

    media.addEventListener("timeupdate", onTimeUpdate);
    media.addEventListener("play", onTimeUpdate);
    media.addEventListener("pause", onPauseOrEnded);
    media.addEventListener("ended", onEndedMedia);
    media.addEventListener("loadedmetadata", onLoadedMetadata);
    media.addEventListener("volumechange", onTimeUpdate);
    if (media.readyState >= HTMLMediaElement.HAVE_METADATA) {
      applyResumeAndPlay(media);
    }
    return () => {
      media.removeEventListener("timeupdate", onTimeUpdate);
      media.removeEventListener("play", onTimeUpdate);
      media.removeEventListener("pause", onPauseOrEnded);
      media.removeEventListener("ended", onEndedMedia);
      media.removeEventListener("loadedmetadata", onLoadedMetadata);
      media.removeEventListener("volumechange", onTimeUpdate);
      flush();
    };
  }, [playback.course, video.mediaUrl, video.video]);

  function togglePlay() {
    const media = videoRef.current;
    if (!media) return;
    if (media.paused) {
      void media.play().catch(() => {
        setPaused(true);
      });
    } else {
      media.pause();
    }
    showChrome();
  }

  function toggleMute() {
    const media = videoRef.current;
    if (!media) return;
    media.muted = !media.muted;
    setMuted(media.muted);
    showChrome();
  }

  function seekTo(nextSeconds: number) {
    const media = videoRef.current;
    if (!media || !Number.isFinite(nextSeconds)) return;
    media.currentTime = Math.max(0, nextSeconds);
    setPositionSeconds(media.currentTime);
    showChrome();
  }

  const durationLabel = formatClock(durationSeconds);
  const positionLabel = formatClock(positionSeconds);
  const seekPercent =
    durationSeconds > 0 ? Math.min(100, Math.max(0, (positionSeconds / durationSeconds) * 100)) : 0;

  return (
    <div
      className="linkedin-player-pane"
      data-chrome={chromeVisible ? "true" : "false"}
      role="region"
      aria-label={video.title}
      onPointerMove={showChrome}
      onPointerEnter={showChrome}
      onPointerLeave={hideChromeNow}
    >
      <video
        ref={videoRef}
        className="linkedin-player-video"
        src={video.mediaUrl ?? undefined}
        playsInline
        preload="auto"
        onClick={togglePlay}
      />
      <div className="linkedin-player-chrome" aria-hidden={!chromeVisible}>
        <div className="linkedin-player-top">
          <button type="button" className="linkedin-player-back" onClick={onClose}>
            Back
          </button>
          <h2>{video.title}</h2>
        </div>
        <div className="linkedin-player-bottom">
          <div className="linkedin-player-seek-wrap">
            <div className="linkedin-player-seek-track" aria-hidden="true">
              <div className="linkedin-player-seek-fill" style={{ width: `${seekPercent}%` }} />
            </div>
            <input
              className="linkedin-player-seek"
              type="range"
              min={0}
              max={durationSeconds > 0 ? durationSeconds : 0}
              step={0.1}
              value={Math.min(positionSeconds, durationSeconds || 0)}
              aria-label="Seek"
              disabled={durationSeconds <= 0}
              onChange={(event) => seekTo(Number(event.target.value))}
            />
          </div>
          <div className="linkedin-player-controls">
            <button
              type="button"
              className="linkedin-player-control"
              aria-label={paused ? "Play" : "Pause"}
              onClick={togglePlay}
            >
              {paused ? <Play aria-hidden="true" /> : <Pause aria-hidden="true" />}
            </button>
            <span className="linkedin-player-time">
              {positionLabel} / {durationLabel}
            </span>
            <button
              type="button"
              className="linkedin-player-control"
              aria-label={muted ? "Unmute" : "Mute"}
              onClick={toggleMute}
            >
              {muted ? <VolumeX aria-hidden="true" /> : <Volume2 aria-hidden="true" />}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function formatClock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const total = Math.floor(seconds);
  const minutes = Math.floor(total / 60);
  const rest = total % 60;
  return `${minutes}:${rest.toString().padStart(2, "0")}`;
}
