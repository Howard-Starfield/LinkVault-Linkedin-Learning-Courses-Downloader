import { invoke } from "@tauri-apps/api/core";

export type CourseUrl = {
  original: string;
  normalized_url: string;
  slug: string;
  quiz_urls: string[];
  assessment_urns: string[];
};

export type LearningUrlRef =
  | ({ kind: "course" } & CourseUrl)
  | {
      kind: "path";
      original: string;
      normalized_url: string;
      path_slug: string;
    }
  | {
      kind: "topic";
      original: string;
      normalized_url: string;
      topic_slug: string;
    };

export type SchedulePolicy = "knownCount" | "discovering";

export type ClassifiedPaste = {
  refs: LearningUrlRef[];
  course_count: number;
  path_count: number;
  topic_count: number;
  schedule_policy: SchedulePolicy;
};

export type ExpansionSummary = {
  paste_ref_count: number;
  path_count: number;
  unique_course_count: number;
  failed_paths: string[];
};

const RESERVED_PREFIXES = ["search", "me", "login", "browse", "in"] as const;

type PreviewClassifyError =
  | { type: "empty" }
  | { type: "notLinkedInLearning"; line: number }
  | { type: "missingSlug"; line: number }
  | { type: "invalidUrl"; line: number }
  | { type: "reservedSegment"; line: number; segment: string };

export function emptyClassifiedPaste(): ClassifiedPaste {
  return {
    refs: [],
    course_count: 0,
    path_count: 0,
    topic_count: 0,
    schedule_policy: "knownCount"
  };
}

export function classifiedPasteFromRefs(refs: LearningUrlRef[]): ClassifiedPaste {
  let course_count = 0;
  let path_count = 0;
  let topic_count = 0;
  for (const learningRef of refs) {
    if (learningRef.kind === "course") course_count += 1;
    else if (learningRef.kind === "path") path_count += 1;
    else topic_count += 1;
  }
  return {
    refs,
    course_count,
    path_count,
    topic_count,
    schedule_policy: path_count > 0 || topic_count > 0 ? "discovering" : "knownCount"
  };
}

export function hasHubRefs(paste: ClassifiedPaste): boolean {
  return paste.schedule_policy === "discovering";
}

export function courseSlugsForHistoryConfirm(paste: ClassifiedPaste): string[] {
  if (hasHubRefs(paste)) return [];
  return paste.refs.flatMap((learningRef) =>
    learningRef.kind === "course" ? [learningRef.slug] : []
  );
}

export function learningRefPreview(learningRef: LearningUrlRef): { heading: string; url: string } {
  if (learningRef.kind === "path") {
    return { heading: "Learning path", url: learningRef.normalized_url };
  }
  if (learningRef.kind === "topic") {
    return { heading: "Topic hub", url: learningRef.normalized_url };
  }
  return { heading: "Ready to queue", url: learningRef.normalized_url };
}

export function knownCourseCount(paste: ClassifiedPaste): number {
  return paste.course_count;
}

export function classifiedPasteToast(paste: ClassifiedPaste): string {
  if (paste.schedule_policy === "discovering") {
    const parts = [
      paste.path_count ? `${paste.path_count} learning path${paste.path_count === 1 ? "" : "s"}` : "",
      paste.topic_count ? `${paste.topic_count} topic${paste.topic_count === 1 ? "" : "s"}` : "",
      paste.course_count ? `${paste.course_count} course${paste.course_count === 1 ? "" : "s"}` : ""
    ].filter(Boolean);
    return `${parts.join(", ")} ready. Unique courses are counted at expand.`;
  }
  return `${paste.course_count} LinkedIn Learning course${paste.course_count === 1 ? "" : "s"} ready to queue.`;
}

export function expansionToastSuffix(summary: ExpansionSummary | null | undefined, base: string): string {
  if (!summary) return base;
  const failed =
    summary.failed_paths.length > 0
      ? ` Failed paths: ${summary.failed_paths.join(", ")}.`
      : "";
  return `${base} Expanded ${summary.unique_course_count} unique course${summary.unique_course_count === 1 ? "" : "s"} from ${summary.paste_ref_count} paste URL${summary.paste_ref_count === 1 ? "" : "s"}.${failed}`;
}

export async function classifyLinkedInLearningUrls(input: string): Promise<ClassifiedPaste> {
  if (isTauriRuntime()) {
    return invoke<ClassifiedPaste>("parse_linkedin_course_urls", { input });
  }
  return classifyLinkedInLearningUrlsForPreview(input);
}

export async function expandLinkedInLearningUrls(
  input: string,
  browserSource?: string
): Promise<ExpansionSummary> {
  if (isTauriRuntime()) {
    return invoke<ExpansionSummary>("expand_linkedin_learning_urls", {
      request: { input, browserSource }
    });
  }
  throw new Error("a LinkedIn Learning session is required to expand paths or topics");
}

export function classifyLinkedInLearningUrlsForPreview(input: string): ClassifiedPaste {
  const refs: LearningUrlRef[] = [];
  for (const [index, rawLine] of input.split(/\r?\n/).entries()) {
    const line = index + 1;
    const candidates = courseUrlCandidates(rawLine);
    if (candidates.length === 0) {
      if (!rawLine.trim()) continue;
      throw previewError({ type: "notLinkedInLearning", line });
    }
    refs.push(...candidates.map((candidate) => classifyLearningUrl(candidate, line)));
  }
  if (refs.length === 0) {
    throw previewError({ type: "empty" });
  }
  return classifiedPasteFromRefs(refs);
}

function isTauriRuntime() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function courseUrlCandidates(line: string): string[] {
  const trimmed = line.trim();
  if (!trimmed) return [];
  const parts = trimmed.split(/\s+/);
  if (parts.length === 1) return [trimCourseUrlToken(parts[0])];
  return parts
    .map(trimCourseUrlToken)
    .filter((part) => part.toLowerCase().includes("linkedin.com/learning/"));
}

function trimCourseUrlToken(token: string) {
  return token.replace(/^[\s"'`<({\[]+|[\s"'`,>)}\]]+$/g, "");
}

function classifyLearningUrl(value: string, line: number): LearningUrlRef {
  const parsed = parseLearningUrl(value, line);
  const prefix = parsed.firstSegment.toLowerCase();
  if (prefix === "paths") {
    const pathSlug = parsed.remaining[0]?.trim();
    if (!pathSlug) throw previewError({ type: "missingSlug", line });
    return {
      kind: "path",
      original: value,
      normalized_url: `https://www.linkedin.com/learning/paths/${pathSlug}`,
      path_slug: pathSlug
    };
  }
  if (prefix === "topics") {
    const topicSlug = parsed.remaining[0]?.trim();
    if (!topicSlug) throw previewError({ type: "missingSlug", line });
    return {
      kind: "topic",
      original: value,
      normalized_url: `https://www.linkedin.com/learning/topics/${topicSlug}`,
      topic_slug: topicSlug
    };
  }
  if ((RESERVED_PREFIXES as readonly string[]).includes(prefix)) {
    throw previewError({ type: "reservedSegment", line, segment: parsed.firstSegment });
  }
  return {
    kind: "course",
    original: value,
    normalized_url: `https://www.linkedin.com/learning/${parsed.firstSegment}`,
    slug: parsed.firstSegment,
    quiz_urls: extractQuizUrls(parsed.url, parsed.firstSegment),
    assessment_urns: extractAssessmentUrns(parsed.url)
  };
}

function parseLearningUrl(value: string, line: number): { url: URL; firstSegment: string; remaining: string[] } {
  const withProtocol = value.startsWith("http://") || value.startsWith("https://") ? value : `https://${value}`;
  let url: URL;
  try {
    url = new URL(withProtocol);
  } catch {
    throw previewError({ type: "invalidUrl", line });
  }
  const host = url.hostname.toLowerCase();
  const isLinkedIn = host === "linkedin.com" || host.endsWith(".linkedin.com");
  if (!isLinkedIn) {
    throw previewError({ type: "notLinkedInLearning", line });
  }
  const segments = url.pathname.split("/").filter(Boolean);
  if (segments[0] !== "learning") {
    throw previewError({ type: "notLinkedInLearning", line });
  }
  const firstSegment = segments[1]?.trim();
  if (!firstSegment) {
    throw previewError({ type: "missingSlug", line });
  }
  return { url, firstSegment, remaining: segments.slice(2) };
}

function extractQuizUrls(url: URL, slug: string): string[] {
  const segments = url.pathname.split("/").filter(Boolean);
  const quizIndex = segments.findIndex((segment) => segment.toLowerCase() === "quiz");
  const assessment = quizIndex >= 0 ? segments[quizIndex + 1] : undefined;
  if (!assessment) return [];
  const normalized = new URL(`https://www.linkedin.com/learning/${slug}/quiz/${assessment}`);
  for (const key of ["resume", "u"]) {
    const value = url.searchParams.get(key);
    if (value) normalized.searchParams.set(key, value);
  }
  return [normalized.toString()];
}

function extractAssessmentUrns(url: URL): string[] {
  const segments = url.pathname.split("/").filter(Boolean);
  return segments
    .flatMap((segment, index) => (segment.toLowerCase() === "quiz" ? [segments[index + 1]] : []))
    .filter((segment): segment is string => Boolean(segment))
    .filter(
      (segment) =>
        segment.startsWith("urn:li:learningApiAssessment:") ||
        segment.startsWith("urn%3Ali%3AlearningApiAssessment%3A")
    );
}

function previewError(error: PreviewClassifyError): Error {
  if (error.type === "empty") {
    return new Error("no LinkedIn Learning course URLs were provided");
  }
  if (error.type === "notLinkedInLearning") {
    return new Error(`line ${error.line}: expected a linkedin.com/learning course URL`);
  }
  if (error.type === "missingSlug") {
    return new Error(`line ${error.line}: missing course slug`);
  }
  if (error.type === "reservedSegment") {
    return new Error(
      `line ${error.line}: '${error.segment}' is a LinkedIn Learning listing or account page, not a course`
    );
  }
  return new Error(`line ${error.line}: could not parse URL`);
}
