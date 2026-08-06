import type { BlockSearchResult, SearchField, SearchHighlight } from "../backend/contracts";

export interface HighlightSegment {
  highlighted: boolean;
  text: string;
}

export function highlightedSegments(
  text: string,
  highlights: SearchHighlight[],
  fields: readonly SearchField[],
): HighlightSegment[] {
  const characters = [...text];
  const allowedFields = new Set(fields);
  const ranges = highlights
    .filter((highlight) => allowedFields.has(highlight.field))
    .map((highlight) => ({
      start: Math.max(0, Math.min(characters.length, highlight.startChar)),
      end: Math.max(0, Math.min(characters.length, highlight.endChar)),
    }))
    .filter((range) => range.start < range.end)
    .sort((left, right) => left.start - right.start || left.end - right.end);
  const merged: Array<{ start: number; end: number }> = [];
  for (const range of ranges) {
    const previous = merged[merged.length - 1];
    if (previous && range.start <= previous.end) {
      previous.end = Math.max(previous.end, range.end);
    } else {
      merged.push({ ...range });
    }
  }
  if (merged.length === 0) {
    return text ? [{ highlighted: false, text }] : [];
  }

  const segments: HighlightSegment[] = [];
  let offset = 0;
  for (const range of merged) {
    if (range.start > offset) {
      segments.push({
        highlighted: false,
        text: characters.slice(offset, range.start).join(""),
      });
    }
    segments.push({
      highlighted: true,
      text: characters.slice(range.start, range.end).join(""),
    });
    offset = range.end;
  }
  if (offset < characters.length) {
    segments.push({
      highlighted: false,
      text: characters.slice(offset).join(""),
    });
  }
  return segments;
}

export function resultHighlights(result: BlockSearchResult | undefined): SearchHighlight[] {
  return result?.highlights ?? [];
}
