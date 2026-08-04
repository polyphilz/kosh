import { withoutKoshStructureMarkers } from "../markdown/structureMarkers";

export function hasMeaningfulAuthoredContent(markdown: string): boolean {
  const mediaAware = withoutKoshStructureMarkers(markdown).replace(
    /\{\{kosh:(?:image|attachment|pdf):[^{}\r\n]+\}\}/gu,
    "media",
  );
  if (/<(?:[A-Za-z][A-Za-z\d+.-]*:[^<>\s]+|[^<>\s@]+@[^<>\s@]+)>/u.test(mediaAware)) {
    return true;
  }
  if (hasMeaningfulCode(mediaAware)) return true;
  const withoutTags = mediaAware
    .split(/\r?\n/u)
    .filter((line) => !/^ {0,3}(?:`{3,}|~{3,}).*$/u.test(line))
    .join("\n")
    .replace(/<[^>]*>/gu, "");
  return withoutTags.replace(/[`*_#>+\-[\]()~$\\\s]/gu, "").length > 0;
}

function hasMeaningfulCode(markdown: string): boolean {
  let fence: { character: "`" | "~"; length: number; hasContent: boolean } | null = null;
  for (const line of markdown.split(/\r?\n/u)) {
    if (fence) {
      const closing = /^ {0,3}(`{3,}|~{3,})[\t ]*$/u.exec(line);
      if (closing?.[1]?.[0] === fence.character && closing[1].length >= fence.length) {
        if (fence.hasContent) return true;
        fence = null;
      } else if (line.trim().length > 0) {
        fence.hasContent = true;
      }
      continue;
    }

    const opening = /^ {0,3}(`{3,}|~{3,}).*$/u.exec(line);
    if (opening?.[1]) {
      fence = {
        character: opening[1][0] as "`" | "~",
        length: opening[1].length,
        hasContent: false,
      };
    } else if (/^(?: {4}|\t)\S/u.test(line)) {
      return true;
    }
  }
  return fence?.hasContent ?? false;
}
