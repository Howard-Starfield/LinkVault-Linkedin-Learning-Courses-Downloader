//! Persisted preferences for the newspaper image-optimization governor.
//!
//! These values are sent to the Rust backend as part of
//! `OptimizationRunOptions` and override the governor's built-in defaults
//! (160 MB per worker, 4 GB reserve). They are intentionally separate from
//! the reader preferences — the reader is about *display*, the governor
//! knobs are about *resource pressure on the rest of the machine*.

export const NEWSPAPER_OPTIMIZATION_PREFERENCES_STORAGE_KEY =
  "linkvault.newspaper.optimizationPreferences";

export type NewspaperOptimizationPreferences = {
  /// Per-worker memory budget in megabytes. 4K and other memory-hungry
  /// editions may want 256-512 MB to avoid swap pressure on large pages.
  workerMemoryBudgetMb: number;
  /// Bytes the system should keep free for the rest of the OS and the
  /// LinkVault UI. Larger values cap the optimization at fewer workers
  /// to leave room for downloads, the reader, and the rest of the app.
  memoryReserveMb: number;
};

export const NEWSPAPER_OPTIMIZATION_PREFERENCES_DEFAULTS: NewspaperOptimizationPreferences = {
  workerMemoryBudgetMb: 160,
  memoryReserveMb: 4096,
};

export const NEWSPAPER_OPTIMIZATION_MEMORY_BOUNDS = {
  workerMemoryBudgetMb: { min: 64, max: 1024, step: 16 },
  memoryReserveMb: { min: 512, max: 32_768, step: 256 }
} as const;

function clampToBounds(
  value: number,
  bounds: { min: number; max: number },
): number {
  if (!Number.isFinite(value)) return bounds.min;
  return Math.min(bounds.max, Math.max(bounds.min, Math.round(value)));
}

export function readNewspaperOptimizationPreferences(): NewspaperOptimizationPreferences {
  if (typeof window === "undefined") {
    return NEWSPAPER_OPTIMIZATION_PREFERENCES_DEFAULTS;
  }
  const raw = window.localStorage.getItem(NEWSPAPER_OPTIMIZATION_PREFERENCES_STORAGE_KEY);
  if (!raw) return NEWSPAPER_OPTIMIZATION_PREFERENCES_DEFAULTS;
  try {
    const parsed = JSON.parse(raw) as Partial<NewspaperOptimizationPreferences>;
    return {
      workerMemoryBudgetMb: clampToBounds(
        Number(parsed.workerMemoryBudgetMb ?? NEWSPAPER_OPTIMIZATION_PREFERENCES_DEFAULTS.workerMemoryBudgetMb),
        NEWSPAPER_OPTIMIZATION_MEMORY_BOUNDS.workerMemoryBudgetMb,
      ),
      memoryReserveMb: clampToBounds(
        Number(parsed.memoryReserveMb ?? NEWSPAPER_OPTIMIZATION_PREFERENCES_DEFAULTS.memoryReserveMb),
        NEWSPAPER_OPTIMIZATION_MEMORY_BOUNDS.memoryReserveMb,
      ),
    };
  } catch {
    return NEWSPAPER_OPTIMIZATION_PREFERENCES_DEFAULTS;
  }
}

export function writeNewspaperOptimizationPreferences(preferences: NewspaperOptimizationPreferences) {
  if (typeof window === "undefined") return;
  const sanitized: NewspaperOptimizationPreferences = {
    workerMemoryBudgetMb: clampToBounds(
      preferences.workerMemoryBudgetMb,
      NEWSPAPER_OPTIMIZATION_MEMORY_BOUNDS.workerMemoryBudgetMb,
    ),
    memoryReserveMb: clampToBounds(
      preferences.memoryReserveMb,
      NEWSPAPER_OPTIMIZATION_MEMORY_BOUNDS.memoryReserveMb,
    ),
  };
  window.localStorage.setItem(
    NEWSPAPER_OPTIMIZATION_PREFERENCES_STORAGE_KEY,
    JSON.stringify(sanitized),
  );
}
