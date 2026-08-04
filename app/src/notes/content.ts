import { withoutKoshStructureMarkers } from "../markdown/structureMarkers";

export function hasMeaningfulAuthoredContent(markdown: string): boolean {
  const mediaAware = withoutKoshStructureMarkers(markdown).replace(
    /\{\{kosh:(?:image|attachment|pdf):[^{}\r\n]+\}\}/gu,
    "media",
  );
  if (/<(?:[A-Za-z][A-Za-z\d+.-]*:[^<>\s]+|[^<>\s@]+@[^<>\s@]+)>/u.test(mediaAware)) {
    return true;
  }
  const withoutTags = mediaAware.replace(/<[^>]*>/gu, "");
  return withoutTags.replace(/[`*_#>+\-[\]()~$\\\s]/gu, "").length > 0;
}
