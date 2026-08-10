import type { EstimatedCropSize } from "./newspaper-clipping-geometry";
import type { NormalizedCropRect } from "./newspaper-api";

export type ClippingTarget = {
  pageId: string;
  pageIndex: number;
  expectedMediaVersion: number;
  rect: NormalizedCropRect;
  estimatedSize: EstimatedCropSize | null;
};

export type NewspaperClippingInteraction =
  | { type: "browse"; announcement?: string }
  | { type: "clip-selecting"; announcement: string }
  | ({ type: "clip-drawing"; pointerId: number } & ClippingTarget)
  | ({ type: "clip-confirming"; operationId?: string; error?: string; requiresRedraw?: boolean } & ClippingTarget)
  | ({ type: "clip-saving"; operationId: string; waiting: boolean } & ClippingTarget);

export type NewspaperClippingAction =
  | { type: "ENTER" }
  | { type: "CANCEL"; announcement?: string }
  | ({ type: "START"; pointerId: number } & ClippingTarget)
  | { type: "DRAW"; rect: NormalizedCropRect; estimatedSize: EstimatedCropSize | null }
  | { type: "CONFIRM"; rect: NormalizedCropRect; estimatedSize: EstimatedCropSize | null }
  | { type: "REJECT_SMALL" }
  | { type: "REDRAW"; announcement?: string }
  | { type: "SAVE"; operationId: string }
  | { type: "WAITING"; operationId: string }
  | { type: "SAVE_FAILED"; error: string; retainOperationId: boolean }
  | ({ type: "REFRESHED"; announcement: string } & ClippingTarget)
  | { type: "SAVED"; announcement: string };

export const initialNewspaperClippingInteraction: NewspaperClippingInteraction = { type: "browse" };

export function newspaperClippingReducer(
  state: NewspaperClippingInteraction,
  action: NewspaperClippingAction
): NewspaperClippingInteraction {
  switch (action.type) {
    case "ENTER":
      return state.type === "browse"
        ? { type: "clip-selecting", announcement: "Clip mode. Drag over one newspaper page." }
        : state;
    case "CANCEL":
      return { type: "browse", announcement: action.announcement ?? "Clipping cancelled." };
    case "START":
      return state.type === "clip-selecting"
        ? { ...action, type: "clip-drawing" }
        : state;
    case "DRAW":
      return state.type === "clip-drawing"
        ? { ...state, rect: action.rect, estimatedSize: action.estimatedSize }
        : state;
    case "CONFIRM":
      return state.type === "clip-drawing"
        ? {
            type: "clip-confirming",
            pageId: state.pageId,
            pageIndex: state.pageIndex,
            expectedMediaVersion: state.expectedMediaVersion,
            rect: action.rect,
            estimatedSize: action.estimatedSize
          }
        : state;
    case "REJECT_SMALL":
      return state.type === "clip-drawing"
        ? { type: "clip-selecting", announcement: "Select a larger area." }
        : state;
    case "REDRAW":
      return state.type === "clip-confirming"
        ? {
            type: "clip-selecting",
            announcement: action.announcement ?? "Drag a new clipping area."
          }
        : state;
    case "SAVE":
      return state.type === "clip-confirming" && !state.requiresRedraw
        ? { ...state, type: "clip-saving", operationId: action.operationId, waiting: false }
        : state;
    case "WAITING":
      return state.type === "clip-saving" && state.operationId === action.operationId
        ? { ...state, waiting: true }
        : state;
    case "SAVE_FAILED":
      return state.type === "clip-saving"
        ? {
            ...state,
            type: "clip-confirming",
            operationId: action.retainOperationId ? state.operationId : undefined,
            error: action.error,
            requiresRedraw: false
          }
        : state;
    case "REFRESHED":
      return state.type === "clip-saving"
        ? {
            ...action,
            type: "clip-confirming",
            error: action.announcement,
            requiresRedraw: true
          }
        : state;
    case "SAVED":
      return state.type === "clip-saving"
        ? { type: "browse", announcement: action.announcement }
        : state;
    default:
      return state;
  }
}

export function clippingModeName(state: NewspaperClippingInteraction) {
  switch (state.type) {
    case "clip-selecting": return "selecting";
    case "clip-drawing": return "drawing";
    case "clip-confirming": return "confirming";
    case "clip-saving": return "saving";
    default: return "browse";
  }
}
