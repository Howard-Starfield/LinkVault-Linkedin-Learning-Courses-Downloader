export const NEWSPAPER_READER_ZOOM_MIN = .5;
export const NEWSPAPER_READER_ZOOM_MAX = 3;
export const NEWSPAPER_READER_ZOOM_STEP = .1;
export const DEFAULT_NEWSPAPER_READER_ZOOM = 1;
export const DEFAULT_NEWSPAPER_CLICK_ZOOM = 1.2;
export const NEWSPAPER_READER_PREFERENCES_EVENT = "linkvault:newspaper-reader-preferences";

export const NEWSPAPER_PAGE_TONES = ["original", "soft", "dim", "inverted"] as const;
export type NewspaperPageTone = typeof NEWSPAPER_PAGE_TONES[number];

export type NewspaperReaderPreferences = {
  defaultZoom: number;
  clickZoom: number;
  pageTone: NewspaperPageTone;
};

const NEWSPAPER_READER_PREFERENCES_KEY = "linkvault.newspaper.reader.preferences";
const NEWSPAPER_READER_PREFERENCES_VERSION = 3;

export function clampNewspaperReaderZoom(value: number) {
  if (!Number.isFinite(value)) return DEFAULT_NEWSPAPER_READER_ZOOM;
  const stepped = Math.round(value / NEWSPAPER_READER_ZOOM_STEP) * NEWSPAPER_READER_ZOOM_STEP;
  return Math.max(NEWSPAPER_READER_ZOOM_MIN, Math.min(NEWSPAPER_READER_ZOOM_MAX, stepped));
}

export function readNewspaperReaderPreferences(): NewspaperReaderPreferences {
  const fallback: NewspaperReaderPreferences = {
    defaultZoom: DEFAULT_NEWSPAPER_READER_ZOOM,
    clickZoom: DEFAULT_NEWSPAPER_CLICK_ZOOM,
    pageTone: "soft"
  };
  if (typeof window === "undefined") return fallback;
  try {
    const stored = JSON.parse(window.localStorage.getItem(NEWSPAPER_READER_PREFERENCES_KEY) ?? "{}") as {
      version?: unknown;
      defaultZoom?: unknown;
      clickZoom?: unknown;
      pageTone?: unknown;
    };
    return {
      defaultZoom: clampNewspaperReaderZoom(
        typeof stored.defaultZoom === "number"
          ? stored.defaultZoom
          : fallback.defaultZoom
      ),
      clickZoom: clampNewspaperReaderZoom(
        typeof stored.clickZoom === "number" ? stored.clickZoom : fallback.clickZoom
      ),
      pageTone: typeof stored.pageTone === "string"
        && NEWSPAPER_PAGE_TONES.includes(stored.pageTone as NewspaperPageTone)
        ? stored.pageTone as NewspaperPageTone
        : fallback.pageTone
    };
  } catch {
    return fallback;
  }
}

export function writeNewspaperReaderPreferences(preferences: NewspaperReaderPreferences) {
  if (typeof window === "undefined") return;
  try {
    const normalized: NewspaperReaderPreferences = {
      defaultZoom: clampNewspaperReaderZoom(preferences.defaultZoom),
      clickZoom: clampNewspaperReaderZoom(preferences.clickZoom),
      pageTone: preferences.pageTone
    };
    window.localStorage.setItem(NEWSPAPER_READER_PREFERENCES_KEY, JSON.stringify({
      version: NEWSPAPER_READER_PREFERENCES_VERSION,
      ...normalized
    }));
    window.dispatchEvent(new CustomEvent(NEWSPAPER_READER_PREFERENCES_EVENT, { detail: normalized }));
  } catch {
    // The reader remains usable when browser storage is unavailable.
  }
}
