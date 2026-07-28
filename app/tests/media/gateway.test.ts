import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  attachmentMediaUrl,
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

  it("constructs only attachment-ID media URLs", () => {
    const id = "019f547b-6200-7000-8000-000000000123";
    expect(attachmentMediaUrl(id)).toBe(`kosh-media://localhost/attachment/${id}`);
    expect(() => attachmentMediaUrl("../../etc/passwd")).toThrow("UUIDv7");
  });

  it("uses explicit maintenance command boundaries", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(limits).mockResolvedValue({});
    await expect(loadMediaLimits()).resolves.toEqual(limits);
    await scanMediaIntegrity();
    await maintainMedia();
    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["media_limits"],
      ["media_integrity_scan"],
      ["maintain_media"],
    ]);
  });
});
