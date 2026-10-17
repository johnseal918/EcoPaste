/** Delay repeated native preview requests after a failed Win32 window operation. */
export const PREVIEW_OPEN_FAILURE_COOLDOWN_MS = 1_200;

/** Deduplicate in-flight opens for one clipboard card without blocking another card. */
export function canStartPreviewOpen(
  itemId: string,
  inFlightItemId: string | null,
  retryAfterMs: number,
  nowMs: number,
): boolean {
  return itemId !== inFlightItemId && nowMs >= retryAfterMs;
}
