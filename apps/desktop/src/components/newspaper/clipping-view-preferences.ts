export const CLIPPING_VIEW_MODE_STORAGE_KEY = "linkvault.newspaper.clippingsViewMode";
export const CLIPPING_VIEW_MODE_EVENT = "linkvault:newspaper-clippings-view-mode";

export type ClippingViewMode = "gallery" | "list";

export const CLIPPING_VIEW_MODE_DEFAULT: ClippingViewMode = "gallery";

export function isClippingViewMode(value: unknown): value is ClippingViewMode {
  return value === "gallery" || value === "list";
}

export function readClippingViewMode(): ClippingViewMode {
  if (typeof window === "undefined") return CLIPPING_VIEW_MODE_DEFAULT;
  try {
    const raw = window.localStorage.getItem(CLIPPING_VIEW_MODE_STORAGE_KEY);
    if (isClippingViewMode(raw)) return raw;
  } catch {
    return CLIPPING_VIEW_MODE_DEFAULT;
  }
  return CLIPPING_VIEW_MODE_DEFAULT;
}

export function writeClippingViewMode(mode: ClippingViewMode): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(CLIPPING_VIEW_MODE_STORAGE_KEY, mode);
  } catch {
    // Still publish so the UI can update even when storage is unavailable.
  }
  window.dispatchEvent(new CustomEvent<ClippingViewMode>(CLIPPING_VIEW_MODE_EVENT, { detail: mode }));
}
