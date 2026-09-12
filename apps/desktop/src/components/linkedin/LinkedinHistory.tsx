import { useEffect, useMemo, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import { ArrowLeft, FolderOpen } from "lucide-react";
import { Button } from "../primitives";
import { LinkedinVirtualList } from "./LinkedinVirtualList";
import { LINKEDIN_HISTORY_ROW_PX } from "../../lib/linkedin/browse-window";
import {
  linkedinAddCourseToPath,
  linkedinListCatalog,
  linkedinOpenCourse,
  linkedinOpenCourseFolder
} from "../../lib/linkedin/ipc";
import { reduceLibraryNav } from "../../lib/linkedin/nav";
import { nextPlayableVideo } from "../../lib/linkedin/playback";
import type {
  CatalogEntry,
  CoursePlayback,
  CourseSlug,
  LibraryNav,
  PathCatalogEntry,
  PathCourseSummary,
  PathSlug,
  PlayerSession,
  VideoPlayback,
  VideoProgress
} from "../../lib/linkedin/types";
import { LinkedinPlayerPane } from "./LinkedinPlayerPane";
import { CatalogCourseMedia, HistoryProgressRing } from "./MiniCourseArt";

type LinkedinHistoryProps = {
  historyRevision: string;
  onPlayerOpenChange?: (open: boolean) => void;
};

type HistoryMenu =
  | {
      x: number;
      y: number;
      kind: "standalone";
      course: CourseSlug;
      title: string;
      sourceUrl: string;
    }
  | {
      x: number;
      y: number;
      kind: "copy";
      sourceUrl: string;
    };

export function LinkedinHistory({ historyRevision, onPlayerOpenChange }: LinkedinHistoryProps) {
  const [catalog, setCatalog] = useState<CatalogEntry[]>([]);
  const [nav, setNav] = useState<LibraryNav>({ level: "catalog" });
  const [playback, setPlayback] = useState<CoursePlayback | null>(null);
  const [session, setSession] = useState<PlayerSession>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [menu, setMenu] = useState<HistoryMenu | null>(null);
  const browseRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (session) return;
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
  }, [historyRevision, session]);

  useEffect(() => {
    onPlayerOpenChange?.(session !== null);
  }, [onPlayerOpenChange, session]);

  useEffect(() => {
    return () => {
      onPlayerOpenChange?.(false);
    };
  }, [onPlayerOpenChange]);

  useEffect(() => {
    if (!menu) return;
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") setMenu(null);
    }
    function onPointerDown() {
      setMenu(null);
    }
    window.addEventListener("keydown", onKey);
    window.addEventListener("pointerdown", onPointerDown);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pointerdown", onPointerDown);
    };
  }, [menu]);

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
    void linkedinOpenCourse(entry.course)
      .then((next) => {
        setPlayback(next);
        setLoadError(null);
      })
      .catch((error: unknown) => {
        setPlayback(null);
        setLoadError(error instanceof Error ? error.message : String(error));
      });
  }

  function onOpenPathCourse(course: PathCourseSummary, path: PathCatalogEntry) {
    setNav(reduceLibraryNav(nav, { type: "openCourse", course: course.course, via: { kind: "path", path: path.path } }));
    setSession(null);
    void linkedinOpenCourse(course.course)
      .then((next) => {
        setPlayback(next);
        setLoadError(null);
      })
      .catch((error: unknown) => {
        setPlayback(null);
        setLoadError(error instanceof Error ? error.message : String(error));
      });
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

  function onVideoEnded() {
    if (!playback || !session) return;
    const next = nextPlayableVideo(playback, session.video);
    if (!next) return;
    setSession({ course: playback.course, video: next.video });
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

  const pathEntries = useMemo(
    () => catalog.filter((item): item is PathCatalogEntry => item.kind === "path"),
    [catalog]
  );

  function openMenu(event: ReactMouseEvent, next: HistoryMenu) {
    event.preventDefault();
    event.stopPropagation();
    const maxX = Math.max(8, window.innerWidth - 240);
    const maxY = Math.max(8, window.innerHeight - 12);
    setMenu({
      ...next,
      x: Math.min(Math.max(8, next.x), maxX),
      y: Math.min(Math.max(8, next.y), maxY)
    });
  }

  async function copyUrl(url: string) {
    if (!url.trim()) {
      setMenu(null);
      return;
    }
    try {
      await navigator.clipboard.writeText(url);
    } catch {
      setLoadError("Could not copy the course URL");
    }
    setMenu(null);
  }

  async function moveStandaloneToPath(course: CourseSlug, path: PathSlug) {
    try {
      const next = await linkedinAddCourseToPath(course, path);
      setCatalog(next);
      setLoadError(null);
      setNav({ level: "path", path });
      setPlayback(null);
      setSession(null);
    } catch (error: unknown) {
      setLoadError(error instanceof Error ? error.message : String(error));
    }
    setMenu(null);
  }

  const heading =
    nav.level === "catalog"
      ? "Download history"
      : nav.level === "path"
        ? (activePath?.title ?? "Learning path")
        : (playback?.title ?? "Course");

  const courseRows = useMemo(() => (playback ? flattenCourseRows(playback) : []), [playback]);

  return (
    <div
      className="lv-workspace linkedin-library-workspace"
      data-player-open={session ? "true" : "false"}
      onContextMenu={(event) => event.preventDefault()}
    >
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
      <div className="linkedin-library-stage">
        <div className="linkedin-library-browse" ref={browseRef}>
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
              <LinkedinVirtualList
                items={catalog}
                scrollRef={browseRef}
                ariaLabel="LinkedIn catalog"
                getKey={(entry) => (entry.kind === "path" ? `path:${entry.path}` : `standalone:${entry.course}`)}
                renderItem={(entry) =>
                  entry.kind === "path" ? (
                    <button
                      type="button"
                      className="linkedin-library-row"
                      onClick={() => onOpenEntry(entry)}
                      onContextMenu={(event) =>
                        openMenu(event, {
                          x: event.clientX,
                          y: event.clientY,
                          kind: "copy",
                          sourceUrl: entry.sourceUrl
                        })
                      }
                    >
                      <HistoryProgressRing completed={entry.completedVideos} total={entry.totalVideos} />
                      <span className="linkedin-library-copy">
                        <strong>{entry.title}</strong>
                        <span>{entry.courses.length} course{entry.courses.length === 1 ? "" : "s"}</span>
                      </span>
                    </button>
                  ) : (
                    <button
                      type="button"
                      className="linkedin-library-row"
                      onClick={() => onOpenEntry(entry)}
                      onContextMenu={(event) =>
                        openMenu(event, {
                          x: event.clientX,
                          y: event.clientY,
                          kind: "standalone",
                          course: entry.course,
                          title: entry.title,
                          sourceUrl: entry.sourceUrl
                        })
                      }
                    >
                      <CatalogCourseMedia
                        title={entry.title}
                        thumbnailUrl={entry.thumbnailUrl}
                        completed={entry.completedVideos}
                        total={entry.totalVideos}
                      />
                      <span className="linkedin-library-copy">
                        <strong>{entry.title}</strong>
                        <span>Standalone course</span>
                      </span>
                    </button>
                  )
                }
              />
            )
          ) : nav.level === "path" && activePath ? (
            <LinkedinVirtualList
              items={activePath.courses}
              scrollRef={browseRef}
              ariaLabel={`${activePath.title} courses`}
              getKey={(course) => course.course}
              renderItem={(course) => (
                <button type="button" className="linkedin-library-row" onClick={() => onOpenPathCourse(course, activePath)}>
                  <CatalogCourseMedia
                    title={course.title}
                    thumbnailUrl={course.thumbnailUrl}
                    completed={course.completedVideos}
                    total={course.totalVideos}
                  />
                  <span className="linkedin-library-copy">
                    <strong>{course.title}</strong>
                    <span>{jobStatusLabel(course.jobStatus)}</span>
                  </span>
                </button>
              )}
            />
          ) : playback ? (
            <LinkedinVirtualList
              items={courseRows}
              scrollRef={browseRef}
              ariaLabel={`${playback.title} videos`}
              getKey={(row) => row.key}
              estimateSize={(index) => (courseRows[index]?.kind === "chapter" ? 32 : LINKEDIN_HISTORY_ROW_PX)}
              renderItem={(row) =>
                row.kind === "chapter" ? (
                  <h3 className="linkedin-library-chapter-title">{row.title}</h3>
                ) : (
                  <button
                    type="button"
                    className="linkedin-library-row"
                    onClick={() => onOpenVideo(row.video)}
                    disabled={!row.video.mediaUrl}
                  >
                    <HistoryProgressRing completed={row.video.progress.completedAt !== null ? 1 : 0} total={1} />
                    <span className="linkedin-library-copy">
                      <strong>{row.video.title}</strong>
                      <span>{row.video.mediaUrl ? (row.video.progress.completedAt !== null ? "Complete" : "Ready") : "Downloading"}</span>
                    </span>
                  </button>
                )
              }
            />
          ) : (
            <div className="linkedin-library-empty" role="status">
              <span>Loading course</span>
            </div>
          )}
        </div>
        {playback ? (
          <LinkedinPlayerPane
            playback={playback}
            session={session}
            onClose={() => setSession(null)}
            onProgress={onProgress}
            onEnded={onVideoEnded}
          />
        ) : null}
      </div>
      {menu ? (
        <div
          className="linkedin-library-menu"
          role="menu"
          style={{ left: menu.x, top: menu.y }}
          onPointerDown={(event) => event.stopPropagation()}
        >
          {menu.kind === "standalone" ? (
            <>
              {pathEntries.length === 0 ? (
                <button type="button" className="linkedin-library-menu-item" role="menuitem" disabled>
                  No learning paths yet
                </button>
              ) : (
                pathEntries.map((path) => (
                  <button
                    key={path.path}
                    type="button"
                    className="linkedin-library-menu-item"
                    role="menuitem"
                    onClick={() => void moveStandaloneToPath(menu.course, path.path)}
                  >
                    Move to {path.title}
                  </button>
                ))
              )}
              <button
                type="button"
                className="linkedin-library-menu-item"
                role="menuitem"
                disabled={!menu.sourceUrl.trim()}
                onClick={() => void copyUrl(menu.sourceUrl)}
              >
                Copy URL
              </button>
            </>
          ) : (
            <button
              type="button"
              className="linkedin-library-menu-item"
              role="menuitem"
              disabled={!menu.sourceUrl.trim()}
              onClick={() => void copyUrl(menu.sourceUrl)}
            >
              Copy URL
            </button>
          )}
        </div>
      ) : null}
    </div>
  );
}

function flattenCourseRows(playback: CoursePlayback): CourseBrowseRow[] {
  return playback.chapters.flatMap((chapter) => [
    { kind: "chapter", key: `chapter:${chapter.title}`, title: chapter.title },
    ...chapter.videos.map((video) => ({ kind: "video" as const, key: `video:${video.video}`, video }))
  ]);
}

type CourseBrowseRow =
  | { kind: "chapter"; key: string; title: string }
  | { kind: "video"; key: string; video: VideoPlayback };

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

