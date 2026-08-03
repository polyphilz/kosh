export const MIN_IMAGE_WIDTH_PERCENT = 10;
export const MAX_IMAGE_WIDTH_PERCENT = 100;

export function initialImageWidth(naturalWidth: number, editorWidth: number): number {
  if (editorWidth <= 0 || naturalWidth >= editorWidth) {
    return MAX_IMAGE_WIDTH_PERCENT;
  }
  return clampImageWidth((naturalWidth / editorWidth) * 100);
}

export function clampImageWidth(value: number): number {
  return Math.max(MIN_IMAGE_WIDTH_PERCENT, Math.min(MAX_IMAGE_WIDTH_PERCENT, Math.round(value)));
}
