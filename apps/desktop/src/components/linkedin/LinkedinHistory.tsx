import { useEffect, useMemo, useState } from "react";
import { ArrowLeft, FolderOpen } from "lucide-react";
import { Button } from "../primitives";
import {
  linkedinListCatalog,
  linkedinOpenCourse,
  linkedinOpenCourseFolder
} from "../../lib/linkedin/ipc";
import { reduceLibraryNav } from "../../lib/linkedin/nav";
import type {
  CatalogEntry,
  CoursePlayback,
  CourseSlug,
  LibraryNav,
  PathCatalogEntry,
  PathCourseSummary,
  PlayerSession,
  VideoPlayback,
  VideoProgress
} from "../../lib/linkedin/types";
import { LinkedinPlayer } from "./LinkedinPlayer";

type LinkedinHistoryProps = {
  historyRevision: string;
};

export function LinkedinHistory({ historyRevision }: LinkedinHistoryProps) {
  const [catalog, setCatalog] = useState<CatalogEntry[]>([]);
  const [nav, setNav] = useState<LibraryNav>({ level: "catalog" });
  const [playback, setPlayback] = useState<CoursePlayback | null>(null);
  const [session, setSession] = useState<PlayerSession>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void linkedinListCatalog()
      .then((entries) => {
        if (!cancelled) {
          setCatalog(entries);
          setLoadError(null);
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setLoadError(error instanceof Error ? error.message : String(error));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [historyRevision]);

  const activePath = useMemo(() => {
    if (nav.level === "path") {
      return catalog.find((item): item is PathCatalogEntry => item.kind === "path" && item.path === nav.path) ?? null;
    }
    if (nav.level === "course" && nav.via.kind === "path") {
      const pathSlug = nav.via.path;
      return catalog.find((item): item is PathCatalogEntry => item.kind === "path" && item.path === pathSlug) ?? null;
    }
    return null;
  }, [catalog, nav]);

  function onOpenEntry(entry: CatalogEntry) {
    if (entry.kind === "path") {
      setNav(reduceLibraryNav(nav, { type: "openPath", path: entry.path }));
      setPlayback(null);
      setSession(null);
      return;
    }
    setNav(reduceLibraryNav(nav, { type: "openCourse", course: entry.course, via: { kind: "standalone" } }));
    setSession(null);
    void linkedinOpenCourse(entry.course).then(setPlayback);
  }

  function onOpenPathCourse(course: PathCourseSummary, path: PathCatalogEntry) {
    setNav(reduceLibraryNav(nav, { type: "openCourse", course: course.course, via: { kind: "path", path: path.path } }));
    setSession(null);
    void linkedinOpenCourse(course.course).then(setPlayback);
  }

  function onBack() {
    const next = reduceLibraryNav(nav, { type: "back" });
    setNav(next);
    setSession(null);
    if (next.level !== "course") {
      setPlayback(null);
    }
  }

  function onOpenVideo(video: VideoPlayback) {
    if (!playback || !video.mediaUrl) return;
    setSession({ course: playback.course, video: video.video });
  }

  function onProgress(video: VideoPlayback, progress: VideoProgress) {
    setPlayback((current) => {
      if (!current) return current;
      return {
        ...current,
        chapters: current.chapters.map((chapter) => ({
          ...chapter,
          videos: chapter.videos.map((item) =>
            item.video === video.video ? { ...item, progress } : item
          )
        }))
      };
    });
  }

  async function onOpenFolder(course: CourseSlug) {
    await linkedinOpenCourseFolder(course);
  }

  const heading =
    nav.level === "catalog"
      ? "Download history"
      : nav.level === "path"
        ? (activePath?.title ?? "Learning path")
        : (playback?.title ?? "Course");

  return (
    <div className="lv-workspace linkedin-library-workspace">
      <div className="linkedin-library-header">
        {nav.level !== "catalog" ? (
          <Button type="button" size="xs" variant="ghost" onClick={onBack} aria-label="Back">
            <ArrowLeft aria-hidden="true" />
            Back
          </Button>
        ) : null}
        <p className="linkedin-library-title">{heading}</p>
        {nav.level === "course" && playback ? (
          <Button
            type="button"
            size="xs"
            variant="ghost"
            onClick={() => void onOpenFolder(playback.course)}
            aria-label={`Open folder for ${playback.title}`}
          >
            <FolderOpen aria-hidden="true" />
            Open Folder
          </Button>
        ) : null}
      </div>
      {loadError ? (
        <div className="linkedin-library-empty" role="status">
          <span>Could not load catalog</span>
          <span>{loadError}</span>
        </div>
      ) : nav.level === "catalog" ? (
        catalog.length === 0 ? (
          <div className="linkedin-library-empty" role="status">
            <span>No saved paths or courses</span>
            <span>Queued LinkedIn paths and pasted courses appear here.</span>
          </div>
        ) : (
          <ol className="linkedin-library-list" aria-label="LinkedIn catalog">
            {catalog.map((entry) =>
              entry.kind === "path" ? (
                <li key={`path:${entry.path}`}>
                  <button type="button" className="linkedin-library-row" onClick={() => onOpenEntry(entry)}>
                    <ProgressRing completed={entry.completedVideos} total={entry.totalVideos} />
                    <span className="linkedin-library-copy">
                      <strong>{entry.title}</strong>
                      <span>{entry.courses.length} course{entry.courses.length === 1 ? "" : "s"}</span>
                    </span>
                  </button>
                </li>
              ) : (
                <li key={`standalone:${entry.course}`}>
                  <button type="button" className="linkedin-library-row" onClick={() => onOpenEntry(entry)}>
                    <ProgressRing completed={entry.completedVideos} total={entry.totalVideos} />
                    <span className="linkedin-library-copy">
                      <strong>{entry.title}</strong>
                      <span>Standalone course</span>
                    </span>
                  </button>
                </li>
              )
            )}
          </ol>
        )
      ) : nav.level === "path" && activePath ? (
        <ol className="linkedin-library-list" aria-label={`${activePath.title} courses`}>
          {activePath.courses.map((course) => (
            <li key={course.course}>
              <button type="button" className="linkedin-library-row" onClick={() => onOpenPathCourse(course, activePath)}>
                <ProgressRing completed={course.completedVideos} total={course.totalVideos} />
                <span className="linkedin-library-copy">
                  <strong>{course.title}</strong>
                  <span>{jobStatusLabel(course.jobStatus)}</span>
                </span>
              </button>
            </li>
          ))}
        </ol>
      ) : playback ? (
        <ol className="linkedin-library-list" aria-label={`${playback.title} videos`}>
          {playback.chapters.map((chapter) => (
            <li key={chapter.title} className="linkedin-library-chapter">
              <h3>{chapter.title}</h3>
              <ol>
                {chapter.videos.map((video) => {
                  const complete = video.progress.completedAt !== null;
                  return (
                    <li key={video.video}>
                      <button
                        type="button"
                        className="linkedin-library-row"
                        onClick={() => onOpenVideo(video)}
                        disabled={!video.mediaUrl}
                      >
                        <ProgressRing completed={complete ? 1 : 0} total={1} />
                        <span className="linkedin-library-copy">
                          <strong>{video.title}</strong>
                          <span>{video.mediaUrl ? (complete ? "Complete" : "Ready") : "Downloading"}</span>
                        </span>
                      </button>
                    </li>
                  );
                })}
              </ol>
            </li>
          ))}
        </ol>
      ) : (
        <div className="linkedin-library-empty" role="status">
          <span>Loading course</span>
        </div>
      )}
      {playback ? (
        <LinkedinPlayer
          playback={playback}
          session={session}
          onClose={() => setSession(null)}
          onProgress={onProgress}
        />
      ) : null}
    </div>
  );
}

function jobStatusLabel(status: PathCourseSummary["jobStatus"]): string {
  switch (status) {
    case "queued":
      return "Queued";
    case "downloading":
      return "Downloading";
    case "completed":
      return "Downloaded";
    case "failed":
      return "Failed";
    case "absent":
      return "Not queued";
    default: {
      const exhaustive: never = status;
      return exhaustive;
    }
  }
}

function ProgressRing({ completed, total }: { completed: number; total: number }) {
  const ratio = total > 0 ? Math.min(1, completed / total) : 0;
  const circumference = 2 * Math.PI * 12;
  const dash = circumference * ratio;
  return (
    <svg className="linkedin-progress-ring" viewBox="0 0 32 32" aria-hidden="true">
      <circle cx="16" cy="16" r="12" className="linkedin-progress-ring-track" />
      <circle
        cx="16"
        cy="16"
        r="12"
        className="linkedin-progress-ring-value"
        strokeDasharray={`${dash} ${circumference}`}
      />
    </svg>
  );
}
