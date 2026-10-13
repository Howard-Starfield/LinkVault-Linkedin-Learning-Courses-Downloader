import { useCallback, useEffect, useRef, useState } from "react";
import { Captions, CaptionsOff, Pause, Play, SkipBack, SkipForward, Volume2, VolumeX } from "lucide-react";
import { saveYouTubePlaybackProgress } from "../../lib/youtube/ipc";
import {
  adjacentPlayableVideo,
  createNavigationGuard,
  createProgressSaver,
  createSerialRunner,
  formatPlaybackClock,
  isPlaybackCompleted,
  progressOfVideo,
  resumePositionSeconds,
  type PlaybackSample,
  type SerialRunner
} from "../../lib/youtube/playback";
import type { YouTubePlaybackProgress, YouTubePlayerSession, YouTubeRunVideo } from "../../lib/youtube/types";

const CHROME_HIDE_MS = 2000;
const CLOCK_UI_MS = 250;
/** Captions sit this far down (percent of the video height) so the control bar never covers them. */
const CAPTION_LINE_PERCENT = 80;

/** Lift every cue off the bottom edge, where the player chrome sits. */
function liftCaptionCues(textTrack: TextTrack) {
  const cues = textTrack.cues;
  if (!cues) return;
  for (let index = 0; index < cues.length; index += 1) {
    const cue = cues[index];
    if (cue instanceof VTTCue) {
      cue.snapToLines = false;
      cue.line = CAPTION_LINE_PERCENT;
      cue.lineAlign = "end";
    }
  }
}

type YouTubePlayerPaneProps = {
  session: YouTubePlayerSession;
  onClose: () => void;
};

/**
 * In-app player for finished YouTube downloads. It deliberately reuses the
 * LinkedIn player's `linkedin-player-*` styles so both look and behave alike;
 * the playback logic is YouTube-specific (opaque media URLs, a CC track, a
 * playlist, resume saved through `saveYouTubePlaybackProgress`).
 */
export function YouTubePlayerPane({ session, onClose }: YouTubePlayerPaneProps) {
  const [currentId, setCurrentId] = useState(session.startOccurrenceId);
  const [captionsOn, setCaptionsOn] = useState(true);
  // Latest known progress per video. Seeded from the listing and updated on
  // every save so moving between videos resumes where each one stopped.
  const progressRef = useRef(new Map<string, YouTubePlaybackProgress>());
  const runnerRef = useRef(createSerialRunner());
  const flushRef = useRef<(() => Promise<void>) | null>(null);
  const closingRef = useRef(false);

  const video = session.videos.find((item) => item.occurrenceId === currentId) ?? null;
  const previous = adjacentPlayableVideo(session.videos, currentId, -1);
  const next = adjacentPlayableVideo(session.videos, currentId, 1);

  const close = useCallback(async () => {
    if (closingRef.current) return;
    closingRef.current = true;
    try {
      await flushRef.current?.();
      // Always wait for every queued save, including those of earlier videos
      // and saves already in flight, before the history reloads progress.
      await runnerRef.current.drain();
    } finally {
      onClose();
    }
  }, [onClose]);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        void close();
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [close]);

  const recordProgress = useCallback((occurrenceId: string, progress: YouTubePlaybackProgress) => {
    progressRef.current.set(occurrenceId, progress);
  }, []);

  if (!video || !video.available || !video.mediaUrl) return null;
  const resumeSeconds = resumePositionSeconds(
    progressRef.current.get(video.occurrenceId) ?? progressOfVideo(video)
  );
  const index = session.videos.findIndex((item) => item.occurrenceId === currentId);

  return (
    <PlayerPane
      key={video.occurrenceId}
      runId={session.runId}
      video={video}
      positionLabel={session.videos.length > 1 ? `${index + 1} of ${session.videos.length}` : null}
      resumeSeconds={resumeSeconds}
      captionsOn={captionsOn}
      onToggleCaptions={() => setCaptionsOn((value) => !value)}
      runner={runnerRef.current}
      registerFlush={(flush) => {
        flushRef.current = flush;
      }}
      onRecord={recordProgress}
      onClose={() => void close()}
      onPrevious={previous ? () => setCurrentId(previous.occurrenceId) : null}
      onNext={
        next
          ? () => {
              if (!closingRef.current) setCurrentId(next.occurrenceId);
            }
          : null
      }
    />
  );
}

type PlayerPaneProps = {
  runId: string;
  video: YouTubeRunVideo;
  positionLabel: string | null;
  resumeSeconds: number;
  captionsOn: boolean;
  onToggleCaptions: () => void;
  runner: SerialRunner;
  registerFlush: (flush: (() => Promise<void>) | null) => void;
  onRecord: (occurrenceId: string, progress: YouTubePlaybackProgress) => void;
  onClose: () => void;
  onPrevious: (() => void) | null;
  onNext: (() => void) | null;
};

function PlayerPane({
  runId,
  video,
  positionLabel,
  resumeSeconds,
  captionsOn,
  onToggleCaptions,
  runner,
  registerFlush,
  onRecord,
  onClose,
  onPrevious,
  onNext
}: PlayerPaneProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const trackRef = useRef<HTMLTrackElement>(null);
  const hideTimerRef = useRef<number | null>(null);
  const onNextRef = useRef(onNext);
  const captionsOnRef = useRef(captionsOn);
  const [chromeVisible, setChromeVisible] = useState(true);
  const [paused, setPaused] = useState(false);
  const [muted, setMuted] = useState(false);
  const [captionsFailed, setCaptionsFailed] = useState(false);
  const [positionSeconds, setPositionSeconds] = useState(resumeSeconds);
  const [durationSeconds, setDurationSeconds] = useState(video.durationSeconds ?? 0);
  onNextRef.current = onNext;
  captionsOnRef.current = captionsOn;
  const hasCaptions = video.subtitlesUrl !== null && !captionsFailed;

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
  }, [video.occurrenceId]);

  // Show or hide the caption track whenever the toggle or the track changes.
  useEffect(() => {
    const track = trackRef.current;
    if (!track) return () => undefined;
    function applyMode() {
      const textTrack = trackRef.current?.track;
      if (textTrack) {
        // "hidden" keeps the cues loaded so toggling back is instant.
        textTrack.mode = captionsOnRef.current ? "showing" : "hidden";
        liftCaptionCues(textTrack);
      }
    }
    function onTrackError() {
      setCaptionsFailed(true);
    }
    applyMode();
    track.addEventListener("load", applyMode);
    track.addEventListener("error", onTrackError);
    return () => {
      track.removeEventListener("load", applyMode);
      track.removeEventListener("error", onTrackError);
    };
  }, [captionsOn, video.subtitlesUrl]);

  useEffect(() => {
    const media = videoRef.current;
    if (!media) {
      return () => undefined;
    }
    let lastSample: PlaybackSample | null = null;
    // Cancelled when this video is left, so a pending "ended" save cannot
    // advance after the user jumped elsewhere or the player closed.
    const navigation = createNavigationGuard();
    const saver = createProgressSaver({
      runner,
      now: () => performance.now(),
      save: (sample) =>
        saveYouTubePlaybackProgress({
          runId,
          occurrenceId: sample.occurrenceId,
          positionSeconds: Math.min(sample.positionSeconds, sample.durationSeconds),
          durationSeconds: sample.durationSeconds
        }),
      onError: (error) => {
        console.error("YouTube playback progress could not be saved", error);
      }
    });

    function persist(force: boolean): Promise<void> {
      const sample = lastSample;
      if (!sample) return runner.drain();
      const position = Math.min(sample.positionSeconds, sample.durationSeconds);
      onRecord(sample.occurrenceId, {
        positionSeconds: position,
        durationSeconds: sample.durationSeconds,
        completed: isPlaybackCompleted(position, sample.durationSeconds)
      });
      return saver.request(sample, force);
    }

    function sampleFrom(target: HTMLVideoElement) {
      if (Number.isFinite(target.currentTime) && Number.isFinite(target.duration) && target.duration > 0) {
        lastSample = {
          occurrenceId: video.occurrenceId,
          positionSeconds: target.currentTime,
          durationSeconds: target.duration
        };
      }
    }

    let primed = false;
    let lastClockMs = 0;

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

    function applyResumeAndPlay(target: HTMLVideoElement) {
      if (primed) return;
      if (!Number.isFinite(target.duration) || target.duration <= 0) return;
      primed = true;
      target.currentTime = resumeSeconds > 0 && resumeSeconds < target.duration ? resumeSeconds : 0;
      syncClock(target, true);
      void target.play().catch(() => {
        setPaused(true);
      });
    }

    function onTimeUpdate(event: Event) {
      const target = event.currentTarget;
      if (!(target instanceof HTMLVideoElement) || !primed) return;
      syncClock(target);
      sampleFrom(target);
      void persist(false);
    }

    function onPause(event: Event) {
      const target = event.currentTarget;
      if (!(target instanceof HTMLVideoElement) || !primed) return;
      syncClock(target, true);
      sampleFrom(target);
      void persist(true);
    }

    function onEndedMedia(event: Event) {
      const target = event.currentTarget;
      if (!(target instanceof HTMLVideoElement) || !primed) return;
      syncClock(target, true);
      sampleFrom(target);
      const stillCurrent = navigation.capture();
      void persist(true).then(() => {
        if (stillCurrent()) onNextRef.current?.();
      });
    }

    function onLoadedMetadata(event: Event) {
      const target = event.currentTarget;
      if (target instanceof HTMLVideoElement) applyResumeAndPlay(target);
    }

    registerFlush(() => {
      if (media.readyState >= HTMLMediaElement.HAVE_METADATA && primed) sampleFrom(media);
      return persist(true);
    });

    media.addEventListener("timeupdate", onTimeUpdate);
    media.addEventListener("play", onTimeUpdate);
    media.addEventListener("pause", onPause);
    media.addEventListener("ended", onEndedMedia);
    media.addEventListener("loadedmetadata", onLoadedMetadata);
    media.addEventListener("volumechange", onTimeUpdate);
    if (media.readyState >= HTMLMediaElement.HAVE_METADATA) {
      applyResumeAndPlay(media);
    }
    return () => {
      media.removeEventListener("timeupdate", onTimeUpdate);
      media.removeEventListener("play", onTimeUpdate);
      media.removeEventListener("pause", onPause);
      media.removeEventListener("ended", onEndedMedia);
      media.removeEventListener("loadedmetadata", onLoadedMetadata);
      media.removeEventListener("volumechange", onTimeUpdate);
      navigation.cancel();
      // Leaving the video (next/previous, close, navigating away) saves the
      // last position once; an unchanged position is skipped by the throttle.
      void persist(true);
      registerFlush(null);
    };
  }, [runId, video.occurrenceId, video.mediaUrl]);

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

  const seekPercent =
    durationSeconds > 0 ? Math.min(100, Math.max(0, (positionSeconds / durationSeconds) * 100)) : 0;

  return (
    <div
      className="linkedin-player-pane youtube-player-pane"
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
        // Captions load from another origin (the youtube-media scheme), and a
        // cross-origin <track> is only fetched when the video opts in to CORS.
        // The protocol handler sends the matching Access-Control-Allow-Origin.
        crossOrigin="anonymous"
        playsInline
        preload="auto"
        onClick={togglePlay}
      >
        {video.subtitlesUrl && !captionsFailed ? (
          <track
            ref={trackRef}
            kind="subtitles"
            src={video.subtitlesUrl}
            label="Captions"
            srcLang="und"
            default
          />
        ) : null}
      </video>
      <div className="linkedin-player-chrome" aria-hidden={!chromeVisible}>
        <div className="linkedin-player-top">
          <button type="button" className="linkedin-player-back" aria-label="Close player" onClick={onClose}>
            Back
          </button>
          <h2>{video.title}</h2>
          {positionLabel ? <span className="youtube-player-position">{positionLabel}</span> : null}
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
            {positionLabel ? (
              <button
                type="button"
                className="linkedin-player-control"
                aria-label="Previous video"
                disabled={onPrevious === null}
                onClick={() => onPrevious?.()}
              >
                <SkipBack aria-hidden="true" />
              </button>
            ) : null}
            <button
              type="button"
              className="linkedin-player-control"
              aria-label={paused ? "Play" : "Pause"}
              onClick={togglePlay}
            >
              {paused ? <Play aria-hidden="true" /> : <Pause aria-hidden="true" />}
            </button>
            {positionLabel ? (
              <button
                type="button"
                className="linkedin-player-control"
                aria-label="Next video"
                disabled={onNext === null}
                onClick={() => onNext?.()}
              >
                <SkipForward aria-hidden="true" />
              </button>
            ) : null}
            <span className="linkedin-player-time">
              {formatPlaybackClock(positionSeconds)} / {formatPlaybackClock(durationSeconds)}
            </span>
            <span className="youtube-player-spacer" aria-hidden="true" />
            {hasCaptions ? (
              <button
                type="button"
                className="linkedin-player-control youtube-player-cc"
                aria-label={captionsOn ? "Turn captions off" : "Turn captions on"}
                aria-pressed={captionsOn}
                onClick={() => {
                  onToggleCaptions();
                  showChrome();
                }}
              >
                {captionsOn ? <Captions aria-hidden="true" /> : <CaptionsOff aria-hidden="true" />}
              </button>
            ) : null}
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
