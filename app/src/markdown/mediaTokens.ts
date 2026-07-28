const UUID_V7 = "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}";
const imagePattern = new RegExp(`^\\{\\{kosh:image:(${UUID_V7});width=(\\d{1,3})%\\}\\}$`, "u");
const attachmentPattern = new RegExp(`^\\{\\{kosh:attachment:(${UUID_V7})\\}\\}$`, "u");

export interface KoshImageToken {
  attachmentId: string;
  kind: "image";
  widthPercent: number;
}

export interface KoshAttachmentToken {
  attachmentId: string;
  kind: "attachment";
}

export type KoshMediaToken = KoshImageToken | KoshAttachmentToken;

export function parseKoshMediaToken(value: string): KoshMediaToken | null {
  const image = imagePattern.exec(value);
  if (image) {
    const widthPercent = Number(image[2]);
    return widthPercent >= 10 && widthPercent <= 100
      ? { attachmentId: image[1]!, kind: "image", widthPercent }
      : null;
  }
  const attachment = attachmentPattern.exec(value);
  return attachment ? { attachmentId: attachment[1]!, kind: "attachment" } : null;
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
  return `{{kosh:image:${token.attachmentId};width=${token.widthPercent}%}}`;
}

export function serializeKoshAttachmentToken(attachmentId: string): string {
  assertUuidV7(attachmentId);
  return `{{kosh:attachment:${attachmentId}}}`;
}

function assertUuidV7(value: string): void {
  if (!new RegExp(`^${UUID_V7}$`, "u").test(value)) {
    throw new Error("media token attachment ID must be a lowercase UUIDv7");
  }
}
