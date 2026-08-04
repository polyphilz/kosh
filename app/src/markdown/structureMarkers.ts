export const EMPTY_BLOCK_MARKER = "<!-- kosh:block:empty -->";
export const CHILDREN_START_MARKER = "<!-- kosh:children:start -->";
export const CHILDREN_END_MARKER = "<!-- kosh:children:end -->";

const KOSH_STRUCTURE_MARKERS = new Set([
  EMPTY_BLOCK_MARKER,
  CHILDREN_START_MARKER,
  CHILDREN_END_MARKER,
]);

export function koshStructureMarker(value: string): string | null {
  const candidate = value.trim();
  return KOSH_STRUCTURE_MARKERS.has(candidate) ? candidate : null;
}

export function withoutKoshStructureMarkers(markdown: string): string {
  return markdown
    .split(/\r?\n/u)
    .filter((line) => koshStructureMarker(line) === null)
    .join("\n");
}
