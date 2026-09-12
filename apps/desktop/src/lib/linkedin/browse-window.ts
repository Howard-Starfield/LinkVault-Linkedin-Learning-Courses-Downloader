/** Typical history viewport (~720px pane) shows about eight 84px rows. */
export const LINKEDIN_HISTORY_ROW_PX = 84;
export const LINKEDIN_HISTORY_OVERSCAN = 1;
export const LINKEDIN_HISTORY_VISIBLE_ROWS = 8;

export function linkedinHistoryMountedLimit(
  visibleRows = LINKEDIN_HISTORY_VISIBLE_ROWS,
  overscan = LINKEDIN_HISTORY_OVERSCAN
): number {
  return Math.max(1, visibleRows) + Math.max(0, overscan) * 2;
}
