const UUID_V7 = "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}";
const imagePattern = new RegExp(
  `^\\{\\{kosh:image:(${UUID_V7});width=(100|[1-9][0-9])%(?:;alt=([^;]+))?(?:;caption=([^;]+))?\\}\\}$`,
  "u",
);
const attachmentPattern = new RegExp(
  `^\\{\\{kosh:attachment:(${UUID_V7})(?:;caption=([^;]+))?\\}\\}$`,
  "u",
);
const pdfPattern = new RegExp(`^\\{\\{kosh:pdf:(${UUID_V7})\\}\\}$`, "u");
const markdownDelimiterPattern = /[!'()*_~]/gu;

export interface KoshImageToken {
  attachmentId: string;
  kind: "image";
  widthPercent: number;
  altText?: string;
  caption?: string;
}

export interface KoshAttachmentToken {
  attachmentId: string;
  kind: "attachment";
  caption?: string;
}

export interface KoshPdfToken {
  attachmentId: string;
  kind: "pdf";
}

export type KoshMediaToken = KoshImageToken | KoshAttachmentToken | KoshPdfToken;

export function neutralizeUntrustedMediaReferences(markdown: string): string {
  const replacements: ReadonlyArray<readonly [string, string]> = [
    ["{{kosh:image:", "{{kosh-reference:image:"],
    ["{{kosh:attachment:", "{{kosh-reference:attachment:"],
    ["{{kosh:pdf:", "{{kosh-reference:pdf:"],
    ["kosh-media://localhost/attachment/", "kosh-reference://localhost/attachment/"],
  ];
  const neutralized = replacements.reduce(
    (value, [from, to]) => value.replaceAll(from, to),
    markdown,
  );
  const decoded = decodeCapabilityEntities(neutralized);
  if (
    [
      "{{kosh:image:",
      "{{kosh:attachment:",
      "{{kosh:pdf:",
      "kosh-media://localhost/attachment/",
    ].some((prefix) => decoded.includes(prefix))
  ) {
    throw new Error("research answer contains an encoded local media capability");
  }
  return neutralized;
}

function decodeCapabilityEntities(value: string): string {
  const named: Record<string, string> = {
    colon: ":",
    lbrace: "{",
    lcub: "{",
    rbrace: "}",
    rcub: "}",
    sol: "/",
  };
  return value
    .replace(/&#(?:x([0-9a-f]+)|([0-9]+));?/giu, (entity, hex: string, decimal: string) => {
      const codePoint = Number.parseInt(hex || decimal, hex ? 16 : 10);
      try {
        return String.fromCodePoint(codePoint);
      } catch {
        return entity;
      }
    })
    .replace(/&(colon|lbrace|lcub|rbrace|rcub|sol);/giu, (entity, name: string) => {
      return named[name.toLowerCase()] ?? entity;
    });
}

export function parseKoshMediaToken(value: string): KoshMediaToken | null {
  const image = imagePattern.exec(value);
  if (image) {
    const widthPercent = Number(image[2]);
    if (widthPercent < 10 || widthPercent > 100) {
      return null;
    }
    const altText = decodeCanonicalField(image[3]);
    const caption = decodeCanonicalField(image[4]);
    if (altText === null || caption === null) {
      return null;
    }
    return {
      attachmentId: image[1]!,
      kind: "image",
      widthPercent,
      ...(altText === undefined ? {} : { altText }),
      ...(caption === undefined ? {} : { caption }),
    };
  }
  const pdf = pdfPattern.exec(value);
  if (pdf) {
    return { attachmentId: pdf[1]!, kind: "pdf" };
  }
  const attachment = attachmentPattern.exec(value);
  if (!attachment) {
    return null;
  }
  const caption = decodeCanonicalField(attachment[2]);
  if (caption === null) {
    return null;
  }
  return {
    attachmentId: attachment[1]!,
    kind: "attachment",
    ...(caption === undefined ? {} : { caption }),
  };
}

export function serializeKoshImageToken(token: Omit<KoshImageToken, "kind">): string {
  assertUuidV7(token.attachmentId);
  if (
    !Number.isInteger(token.widthPercent) ||
    token.widthPercent < 10 ||
    token.widthPercent > 100
  ) {
    throw new Error("image token width must be an integer from 10 to 100");
  }
  const altText = normalizedField(token.altText, 500, "image alt text");
  const caption = normalizedField(token.caption, 2_000, "image caption");
  return [
    `{{kosh:image:${token.attachmentId};width=${token.widthPercent}%`,
    altText ? `;alt=${encodeCanonicalField(altText)}` : "",
    caption ? `;caption=${encodeCanonicalField(caption)}` : "",
    "}}",
  ].join("");
}

export function serializeKoshAttachmentToken(attachmentId: string, caption?: string): string {
  assertUuidV7(attachmentId);
  const normalizedCaption = normalizedField(caption, 2_000, "attachment caption");
  return `{{kosh:attachment:${attachmentId}${
    normalizedCaption ? `;caption=${encodeCanonicalField(normalizedCaption)}` : ""
  }}}`;
}

export function serializeKoshPdfToken(attachmentId: string): string {
  assertUuidV7(attachmentId);
  return `{{kosh:pdf:${attachmentId}}}`;
}

function assertUuidV7(value: string): void {
  if (!new RegExp(`^${UUID_V7}$`, "u").test(value)) {
    throw new Error("media token attachment ID must be a lowercase UUIDv7");
  }
}

function normalizedField(value: string | undefined, limit: number, label: string): string {
  const normalized = value?.trim() ?? "";
  if ([...normalized].length > limit) {
    throw new Error(`${label} must contain at most ${limit} characters`);
  }
  return normalized;
}

function decodeCanonicalField(value: string | undefined): string | undefined | null {
  if (value === undefined) {
    return undefined;
  }
  try {
    const decoded = decodeURIComponent(value);
    return decoded && encodeCanonicalField(decoded) === value ? decoded : null;
  } catch {
    return null;
  }
}

function encodeCanonicalField(value: string): string {
  return encodeURIComponent(value).replace(markdownDelimiterPattern, (character) => {
    const hex = character.codePointAt(0)!.toString(16).toUpperCase();
    return `%${hex}`;
  });
}
