import { catalogMediaKind } from "./catalogMedia";

export { catalogMediaKind };

export function MiniCourseArt(props: { title: string; thumbnailUrl: string }) {
  return (
    <span className="mini-course-art" title={props.title}>
      <img src={props.thumbnailUrl} alt="" loading="lazy" referrerPolicy="no-referrer" />
    </span>
  );
}

export function CatalogCourseMedia(props: {
  title: string;
  thumbnailUrl: string | null;
  completed: number;
  total: number;
}) {
  if (catalogMediaKind(props.thumbnailUrl) === "art" && props.thumbnailUrl !== null) {
    return <MiniCourseArt title={props.title} thumbnailUrl={props.thumbnailUrl} />;
  }
  return <HistoryProgressRing completed={props.completed} total={props.total} />;
}

export function HistoryProgressRing({
  completed,
  total
}: {
  completed: number;
  total: number;
}) {
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
