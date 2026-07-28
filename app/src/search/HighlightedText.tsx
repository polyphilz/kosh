import type { SearchField, SearchHighlight } from "../backend/contracts";
import { highlightedSegments } from "./presentation";

interface HighlightedTextProps {
  fields: readonly SearchField[];
  highlights: SearchHighlight[];
  text: string;
}

export function HighlightedText({ fields, highlights, text }: HighlightedTextProps) {
  return highlightedSegments(text, highlights, fields).map((segment, index) =>
    segment.highlighted ? (
      <mark key={`${index}:${segment.text}`}>{segment.text}</mark>
    ) : (
      segment.text
    ),
  );
}
