import { nextPlayableVideo, playableCourseVideos, resumePlaybackMs } from "./playback";
import { linkedinHistoryMountedLimit } from "./browse-window";
import { asCourseSlug, asVideoSlug } from "./types";
import type { CoursePlayback, LinkedinMediaUrl, VideoPlayback } from "./types";

function video(slug: string, media: boolean): VideoPlayback {
  return {
    video: asVideoSlug(slug),
    title: slug,
    durationMs: 60_000,
    progress: { positionMs: 0, durationMs: 60_000, completedAt: null },
    mediaUrl: media ? (`http://linkedin-media.localhost/v/${slug}` as LinkedinMediaUrl) : null
  };
}

const playback: CoursePlayback = {
  course: asCourseSlug("sample-course"),
  title: "Sample Course",
  chapters: [
    { title: "Intro", videos: [video("welcome", true), video("still-downloading", false)] },
    { title: "Next", videos: [video("chapter-two", true), video("wrap", true)] }
  ]
};

const playable = playableCourseVideos(playback);
if (playable.map((item) => item.video).join(",") !== "welcome,chapter-two,wrap") {
  throw new Error("playableCourseVideos should skip videos without a media URL");
}

const afterWelcome = nextPlayableVideo(playback, asVideoSlug("welcome"));
if (afterWelcome?.video !== "chapter-two") {
  throw new Error("next video should skip a downloading lesson in the same course");
}

const afterChapterTwo = nextPlayableVideo(playback, asVideoSlug("chapter-two"));
if (afterChapterTwo?.video !== "wrap") {
  throw new Error("next video should stay on the same course chapter order");
}

if (nextPlayableVideo(playback, asVideoSlug("wrap")) !== null) {
  throw new Error("the last playable video should not wrap to the start");
}

if (nextPlayableVideo(playback, asVideoSlug("missing")) !== null) {
  throw new Error("an unknown video should not invent a next lesson");
}

const completedNext = video("already-done", true);
completedNext.progress = { positionMs: 59_000, durationMs: 60_000, completedAt: 1_700_000_000 };
const withCompleted: CoursePlayback = {
  ...playback,
  chapters: [
    { title: "Intro", videos: [video("welcome", true), completedNext] }
  ]
};
const afterWelcomeIntoCompleted = nextPlayableVideo(withCompleted, asVideoSlug("welcome"));
if (afterWelcomeIntoCompleted?.video !== "already-done") {
  throw new Error("autoplay should still advance into a completed lesson instead of skipping it");
}
if (resumePlaybackMs(completedNext.progress) !== 0) {
  throw new Error("a completed lesson must start at 0 rather than resuming near the end");
}
if (resumePlaybackMs({ positionMs: 59_400, durationMs: 60_000, completedAt: null }) !== 0) {
  throw new Error("a near-end position must start at 0 so autoplay does not jump to the tail");
}
if (resumePlaybackMs({ positionMs: 12_000, durationMs: 60_000, completedAt: null }) !== 12_000) {
  throw new Error("an in-progress lesson should resume from the saved position");
}
if (linkedinHistoryMountedLimit() !== 10) {
  throw new Error("history browse must mount about 10 rows, not the whole 150+ catalog");
}
