/** Preview panel background only: never change the opacity of text or child controls. */
export function normalizePreviewBackgroundOpacity(value: number): number {
  if (!Number.isFinite(value)) return 100;
  return Math.min(100, Math.max(50, Math.round(value / 5) * 5));
}

/**
 * Fully opaque Ant Design theme color is used verbatim at 100%.
 * Below 100%, only the panel background is alpha-composited.
 * The Tauri overlay outside the card remains transparent.
 */
export function previewPanelBackgroundColor(
  themeColor: string,
  opacity: number,
): string {
  const percent = normalizePreviewBackgroundOpacity(opacity);
  if (percent === 100) return themeColor;
  return `color-mix(in srgb, ${themeColor} ${percent}%, transparent)`;
}
