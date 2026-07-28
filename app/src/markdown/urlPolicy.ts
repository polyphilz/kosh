import type { UrlTransform } from "react-markdown";

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

export const markdownUrlTransform: UrlTransform = (value, key) => {
  if (key !== "href") {
    return "";
  }
  return externalHttpUrl(value) ?? "";
};
