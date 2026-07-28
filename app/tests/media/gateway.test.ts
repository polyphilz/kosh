import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  attachmentMediaUrl,
  ingestAttachmentBytes,
  ingestAttachmentFile,
  loadMediaLimits,
  maintainMedia,
  scanMediaIntegrity,
  type MediaLimits,
} from "../../src/media/gateway";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const limits: MediaLimits = {
  maxAttachmentBytes: 16,
  maxAttachmentsPerDraft: 32,
  maxProtocolResponseBytes: 8,
  draftLeaseDurationMs: 100,
  orphanGracePeriodMs: 200,
  maxReapsPerMaintenance: 4,
};

describe("media gateway", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("frames bounded binary uploads with typed metadata", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(limits).mockResolvedValueOnce({ id: "attachment" });
    const input = {
      draftId: "019f547b-6200-7000-8000-000000000001",
      displayFilename: "note.txt",
      mediaType: "text/plain",
    };
    await ingestAttachmentBytes(input, new Uint8Array([11, 22, 33]));

    expect(vi.mocked(invoke).mock.calls[0]).toEqual(["media_limits"]);
    const [command, body] = vi.mocked(invoke).mock.calls[1] ?? [];
    expect(command).toBe("ingest_attachment");
    expect(body).toBeInstanceOf(Uint8Array);
    const payload = body as Uint8Array;
    const metadataLength = new DataView(
      payload.buffer,
      payload.byteOffset,
      payload.byteLength,
    ).getUint32(0, false);
    expect(JSON.parse(new TextDecoder().decode(payload.subarray(4, 4 + metadataLength)))).toEqual(
      input,
    );
    expect(Array.from(payload.subarray(4 + metadataLength))).toEqual([11, 22, 33]);
  });

  it("rejects empty and oversized bytes before constructing the upload payload", async () => {
    vi.mocked(invoke).mockResolvedValue(limits);

    await expect(
      ingestAttachmentBytes(
        {
          draftId: "draft",
          displayFilename: "empty.txt",
          mediaType: "text/plain",
        },
        new Uint8Array(),
      ),
    ).rejects.toThrow("empty");
    await expect(
      ingestAttachmentBytes(
        {
          draftId: "draft",
          displayFilename: "large.txt",
          mediaType: "text/plain",
        },
        new Uint8Array(17),
      ),
    ).rejects.toThrow("larger than 16 bytes");
    expect(vi.mocked(invoke)).toHaveBeenCalledTimes(2);
  });

  it("checks file metadata before reading bytes into memory", async () => {
    vi.mocked(invoke).mockResolvedValue(limits);
    const arrayBuffer = vi.fn<() => Promise<ArrayBuffer>>();
    const file = {
      arrayBuffer,
      name: "large.pdf",
      size: 17,
      type: "application/pdf",
    } as File;

    await expect(
      ingestAttachmentFile("019f547b-6200-7000-8000-000000000001", file),
    ).rejects.toThrow("larger than 16 bytes");
    expect(arrayBuffer).not.toHaveBeenCalled();
    expect(vi.mocked(invoke).mock.calls).toEqual([["media_limits"]]);
  });

  it("constructs only attachment-ID media URLs", () => {
    const id = "019f547b-6200-7000-8000-000000000123";
    expect(attachmentMediaUrl(id)).toBe(`kosh-media://localhost/attachment/${id}`);
    expect(() => attachmentMediaUrl("../../etc/passwd")).toThrow("UUIDv7");
  });

  it("uses explicit maintenance command boundaries", async () => {
    vi.mocked(invoke).mockResolvedValue({});
    await loadMediaLimits();
    await scanMediaIntegrity();
    await maintainMedia();
    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["media_limits"],
      ["media_integrity_scan"],
      ["maintain_media"],
    ]);
  });
});
