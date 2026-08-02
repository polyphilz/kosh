import { describe, expect, it } from "vitest";
import { selectedAttachmentToMediaBlock } from "../../src/editor/mediaBlocks";
import { clampImageWidth, initialImageWidth } from "../../src/editor/mediaSizing";

describe("BlockNote media blocks", () => {
  it("maps typed local records without serializing leases or byte URLs", () => {
    const image = selectedAttachmentToMediaBlock(
      {
        recordKind: "IMAGE",
        record: {
          id: "019f547b-6200-7000-8000-000000000201",
          ingestLeaseId: "secret-lease",
          displayFilename: "diagram.png",
          mediaType: "image/png",
          byteLength: 100,
          kind: "IMAGE",
          naturalWidth: 400,
          naturalHeight: 300,
          ocrStatus: "READY",
          ocrError: null,
        },
      },
      800,
    );

    expect(image).toMatchObject({
      type: "koshImage",
      props: {
        attachmentId: "019f547b-6200-7000-8000-000000000201",
        widthPercent: 50,
      },
    });
    expect(JSON.stringify(image)).not.toContain("secret-lease");
  });

  it("bounds initial and interactive image sizes", () => {
    expect(initialImageWidth(400, 800)).toBe(50);
    expect(initialImageWidth(1_000, 800)).toBe(100);
    expect(clampImageWidth(-1)).toBe(10);
    expect(clampImageWidth(74.6)).toBe(75);
    expect(clampImageWidth(200)).toBe(100);
  });
});
