import type { UrlTransform } from "react-markdown";

const UUID_V7 = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;

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
  if (key === "src") {
    return localMediaAttachmentId(value) ? value : "";
  }
  return key === "href" ? (externalHttpUrl(value) ?? "") : "";
};

export function localMediaAttachmentId(value: string | undefined): string | null {
  if (!value) {
    return null;
  }
  const prefix = "kosh-media://localhost/attachment/";
  if (!value.startsWith(prefix)) {
    return null;
  }
  const id = value.slice(prefix.length);
  return UUID_V7.test(id) ? id : null;
}
