export function externalHttpUrl(value: string | undefined): string | null {
  if (!value) {
    return null;
  }
  try {
    const url = new URL(value);
    return ["http:", "https:"].includes(url.protocol) && url.host ? url.href : null;
  } catch {
    return null;
  }
}
