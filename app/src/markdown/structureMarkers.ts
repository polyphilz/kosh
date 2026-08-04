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
  let fence: { character: "`" | "~"; length: number } | null = null;
  return markdown
    .split(/\r?\n/u)
    .filter((line) => {
      if (fence) {
        const closing = /^ {0,3}(`{3,}|~{3,})[\t ]*$/u.exec(line);
        if (closing?.[1]?.[0] === fence.character && closing[1].length >= fence.length) {
          fence = null;
        }
        return true;
      }

      const opening = /^ {0,3}(`{3,}|~{3,}).*$/u.exec(line);
      if (opening?.[1]) {
        fence = {
          character: opening[1][0] as "`" | "~",
          length: opening[1].length,
        };
        return true;
      }

      if (/^(?: {4}|\t)/u.test(line)) return true;
      return koshStructureMarker(line) === null;
    })
    .join("\n");
}
