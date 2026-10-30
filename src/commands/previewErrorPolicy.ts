/** Keep actual persistent errors visible, but not once per hover / mousemove. */
export const PREVIEW_HANDLE_TOAST_INTERVAL_MS = 10_000;

export function isTransientPreviewHandleError(
  command: string,
  showPreviewCommand: string,
  message: string,
): boolean {
  return (
    command === showPreviewCommand &&
    /(?:underlying handle is not available|preview native window handle unavailable)/i.test(
      message,
    )
  );
}

export function shouldReportPreviewHandleError(
  command: string,
  showPreviewCommand: string,
  message: string,
  lastShownAt: number,
  now: number,
): boolean {
  return (
    !isTransientPreviewHandleError(command, showPreviewCommand, message) ||
    now - lastShownAt >= PREVIEW_HANDLE_TOAST_INTERVAL_MS
  );
}
