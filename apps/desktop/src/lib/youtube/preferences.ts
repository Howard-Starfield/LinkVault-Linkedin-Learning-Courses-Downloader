import {
  getYouTubePreferences,
  isTauriRuntime,
  saveYouTubePreferences
} from "./ipc";

const LEGACY_OUTPUT_DIR_KEY = "linkvault.youtube.outputDir";

function readStorage(key: string): string {
  if (typeof window === "undefined") return "";
  try {
    const value = window.localStorage.getItem(key);
    return typeof value === "string" ? value : "";
  } catch {
    return "";
  }
}

function writeStorage(key: string, value: string): void {
  if (typeof window === "undefined") return;
  try {
    if (!value.trim()) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, value);
  } catch {
    return;
  }
}

function removeStorage(key: string): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.removeItem(key);
  } catch {
    return;
  }
}

/** Browser-preview only. Production must not treat localStorage as authority. */
export function readPreviewYouTubeOutputDir(): string {
  return readStorage(LEGACY_OUTPUT_DIR_KEY).trim();
}

/** Browser-preview only. */
export function writePreviewYouTubeOutputDir(value: string): void {
  writeStorage(LEGACY_OUTPUT_DIR_KEY, value.trim());
}

function readLegacyYouTubeOutputDir(): string {
  return readStorage(LEGACY_OUTPUT_DIR_KEY).trim();
}

/**
 * Load the persisted YouTube output directory.
 * In Tauri: SQLite `youtube.preferences`. One-shot migrate from legacy
 * localStorage when SQLite is empty. Does not dual-write localStorage.
 * In browser preview: localStorage only.
 */
export async function loadYouTubeOutputDir(): Promise<string> {
  if (!isTauriRuntime()) {
    return readPreviewYouTubeOutputDir();
  }

  const preferences = await getYouTubePreferences();
  const fromDb = preferences.output_dir.trim();
  if (fromDb) return fromDb;

  const legacy = readLegacyYouTubeOutputDir();
  if (!legacy) return "";

  const saved = await saveYouTubePreferences({ output_dir: legacy });
  removeStorage(LEGACY_OUTPUT_DIR_KEY);
  return saved.output_dir.trim();
}

/**
 * Persist a newly chosen output directory.
 * In Tauri: validated SQLite write only (no localStorage).
 * In browser preview: localStorage only.
 */
export async function persistYouTubeOutputDir(outputDir: string): Promise<string> {
  const trimmed = outputDir.trim();
  if (!isTauriRuntime()) {
    writePreviewYouTubeOutputDir(trimmed);
    return trimmed;
  }
  const saved = await saveYouTubePreferences({ output_dir: trimmed });
  return saved.output_dir.trim();
}
