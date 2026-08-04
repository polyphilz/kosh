import type {
  CitationResolution,
  PassageSearchResult,
  SearchField,
  SearchHighlight,
} from "../backend/contracts";

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

export function citationLocation(citation: CitationResolution): string {
  const context = citation.headingContext.filter(Boolean).join(" › ");
  const locator = (() => {
    switch (citation.locator.kind) {
      case "MARKDOWN_BLOCKS": {
        if (citation.locator.startLine !== null && citation.locator.endLine !== null) {
          return rangeLabel("line", citation.locator.startLine, citation.locator.endLine);
        }
        if (citation.locator.startChar !== null && citation.locator.endChar !== null) {
          return `characters ${citation.locator.startChar + 1}–${citation.locator.endChar}`;
        }
        return rangeLabel("block", citation.locator.startBlock + 1, citation.locator.endBlock + 1);
      }
      case "PDF_PAGE":
        return `page ${citation.locator.page}`;
      case "OCR_REGION":
        return citation.locator.page === null
          ? "image region"
          : `page ${citation.locator.page} image region`;
      case "TEXT_LINES":
        return rangeLabel("line", citation.locator.startLine, citation.locator.endLine);
      default:
        return citation.locator satisfies never;
    }
  })();
  return context ? `${context} · ${locator}` : locator;
}

export function citationOwner(citation: CitationResolution): string {
  if (citation.attachment) {
    return citation.attachment.displayFilename;
  }
  return citation.tidbit?.displayTitle ?? "Unknown passage";
}

export function citationRevision(citation: CitationResolution): string {
  const revision = citation.tidbit ? `Revision ${citation.tidbit.revisionNumber}` : "Attachment";
  return `${revision} · ${citation.state === "CURRENT" ? "Current" : "Historical"}`;
}

export function citationCopyText(citation: CitationResolution): string {
  const lines = [
    citationOwner(citation),
    citationLocation(citation),
    citationRevision(citation),
    `Kosh passage: ${citation.passageId}`,
    citation.excerpt,
  ].filter(Boolean);
  return lines.join("\n");
}

export function resultHighlights(result: PassageSearchResult | undefined): SearchHighlight[] {
  return result?.highlights ?? [];
}

function rangeLabel(noun: string, start: number, end: number): string {
  return start === end ? `${noun} ${start}` : `${noun}s ${start}–${end}`;
}
