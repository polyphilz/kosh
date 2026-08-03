const MARKDOWN_INLINE_PUNCTUATION = /([\\`*_[\]{}()<>#+\-.!|])/gu;

export function projectLegacyTitle(title: string | null, bodyMarkdown: string): string {
  const normalized = title?.replace(/\s+/gu, " ").trim();
  if (!normalized) return bodyMarkdown;
  const heading = `# ${normalized.replace(MARKDOWN_INLINE_PUNCTUATION, "\\$1")}`;
  return bodyMarkdown.trim() ? `${heading}\n\n${bodyMarkdown}` : heading;
}
