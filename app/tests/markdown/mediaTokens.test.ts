import { describe, expect, it } from "vitest";
import {
  parseKoshMediaToken,
  serializeKoshAttachmentToken,
  serializeKoshImageToken,
} from "../../src/markdown/mediaTokens";

const imageId = "01980c8e-6c00-7000-8000-000000000201";
const attachmentId = "01980c8e-6c00-7000-8000-000000000202";

describe("reserved Kosh media tokens", () => {
  it("round-trips canonical image and attachment references", () => {
    const image = serializeKoshImageToken({
      attachmentId: imageId,
      widthPercent: 70,
      altText: "Architecture diagram",
      caption: "Chapter 2 / overview",
    });
    const attachment = serializeKoshAttachmentToken(attachmentId);

    expect(image).toBe(
      `{{kosh:image:${imageId};width=70%;alt=Architecture%20diagram;caption=Chapter%202%20%2F%20overview}}`,
    );
    expect(parseKoshMediaToken(image)).toEqual({
      attachmentId: imageId,
      kind: "image",
      widthPercent: 70,
      altText: "Architecture diagram",
      caption: "Chapter 2 / overview",
    });
    expect(parseKoshMediaToken(attachment)).toEqual({
      attachmentId,
      kind: "attachment",
    });
  });

  it("rejects ambiguous IDs, casing, widths, and trailing content", () => {
    expect(parseKoshMediaToken(`{{kosh:image:${imageId};width=9%}}`)).toBeNull();
    expect(parseKoshMediaToken(`{{kosh:image:${imageId};width=010%}}`)).toBeNull();
    expect(parseKoshMediaToken(`{{kosh:image:${imageId};width=070%}}`)).toBeNull();
    expect(parseKoshMediaToken(`{{kosh:image:${imageId.toUpperCase()};width=70%}}`)).toBeNull();
    expect(parseKoshMediaToken(`{{kosh:image:${imageId};width=70%;alt=%ZZ}}`)).toBeNull();
    expect(parseKoshMediaToken(`{{kosh:image:${imageId};width=70%;alt=}}`)).toBeNull();
    expect(
      parseKoshMediaToken(`{{kosh:image:${imageId};width=70%;caption=Caption;alt=Alt}}`),
    ).toBeNull();
    expect(parseKoshMediaToken(`{{kosh:attachment:${attachmentId}}} extra`)).toBeNull();
    expect(() =>
      serializeKoshImageToken({
        attachmentId: imageId,
        widthPercent: 70.5,
      }),
    ).toThrow("integer from 10 to 100");
  });
});
