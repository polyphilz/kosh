import { describe, expect, it } from "vitest";
import {
  neutralizeUntrustedMediaReferences,
  parseKoshMediaToken,
  serializeKoshAttachmentToken,
  serializeKoshImageToken,
  serializeKoshPdfToken,
} from "../../src/markdown/mediaTokens";

const imageId = "01980c8e-6c00-7000-8000-000000000201";
const attachmentId = "01980c8e-6c00-7000-8000-000000000202";

describe("reserved Kosh media tokens", () => {
  it("round-trips canonical image, PDF, and generic attachment references", () => {
    const image = serializeKoshImageToken({
      attachmentId: imageId,
      widthPercent: 70,
      altText: "Architecture diagram",
      caption: "Chapter 2 / overview",
    });
    const attachment = serializeKoshAttachmentToken(attachmentId, "Useful *appendix*");
    const pdf = serializeKoshPdfToken(attachmentId);

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
      caption: "Useful *appendix*",
      kind: "attachment",
    });
    expect(attachment).toBe(`{{kosh:attachment:${attachmentId};caption=Useful%20%2Aappendix%2A}}`);
    expect(pdf).toBe(`{{kosh:pdf:${attachmentId}}}`);
    expect(parseKoshMediaToken(pdf)).toEqual({
      attachmentId,
      kind: "pdf",
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
    expect(parseKoshMediaToken(`{{kosh:attachment:${attachmentId};caption=raw space}}`)).toBeNull();
    expect(parseKoshMediaToken(`{{kosh:pdf:${attachmentId}}} extra`)).toBeNull();
    expect(() =>
      serializeKoshImageToken({
        attachmentId: imageId,
        widthPercent: 70.5,
      }),
    ).toThrow("integer from 10 to 100");
  });

  it("percent-encodes Markdown delimiters in authored image fields", () => {
    const image = serializeKoshImageToken({
      attachmentId: imageId,
      widthPercent: 80,
      altText: "*System* _diagram_",
      caption: "~~Draft~~ (v1)!",
    });

    expect(image).toBe(
      `{{kosh:image:${imageId};width=80%;alt=%2ASystem%2A%20%5Fdiagram%5F;` +
        "caption=%7E%7EDraft%7E%7E%20%28v1%29%21}}",
    );
    expect(parseKoshMediaToken(image)).toEqual({
      attachmentId: imageId,
      kind: "image",
      widthPercent: 80,
      altText: "*System* _diagram_",
      caption: "~~Draft~~ (v1)!",
    });
    expect(parseKoshMediaToken(`{{kosh:image:${imageId};width=80%;alt=*System*}}`)).toBeNull();
  });

  it("neutralizes reserved media capabilities in untrusted Markdown", () => {
    const source = [
      `{{kosh:image:${imageId};width=70%}}`,
      `{{kosh:pdf:${attachmentId}}}`,
      `{{kosh:attachment:${attachmentId}}}`,
      `![direct](kosh-media://localhost/attachment/${attachmentId})`,
    ].join("\n");
    const neutralized = neutralizeUntrustedMediaReferences(source);

    expect(neutralized).not.toContain("{{kosh:image:");
    expect(neutralized).not.toContain("{{kosh:pdf:");
    expect(neutralized).not.toContain("{{kosh:attachment:");
    expect(neutralized).not.toContain("kosh-media://localhost/attachment/");
    expect(neutralized).toContain("{{kosh-reference:image:");
    expect(neutralized).toContain("{{kosh-reference:pdf:");
    expect(neutralized).toContain("{{kosh-reference:attachment:");
    expect(neutralized).toContain("kosh-reference://localhost/attachment/");
  });

  it("rejects entity-encoded media capabilities in untrusted Markdown", () => {
    expect(() =>
      neutralizeUntrustedMediaReferences(`&lcub;&lcub;kosh&#58;image:${imageId};width=70%}}`),
    ).toThrow("encoded local media capability");
    expect(() =>
      neutralizeUntrustedMediaReferences(
        `![direct](kosh-media&#x3a;&sol;&sol;localhost/attachment/${attachmentId})`,
      ),
    ).toThrow("encoded local media capability");
  });
});
