import { describe, expect, it } from "vitest";
import { FakeBackend } from "../../src/backend/fakeBackend";

describe("FakeBackend tidbits", () => {
  it("mirrors create, optimistic edit, listing, and soft deletion", async () => {
    const backend = new FakeBackend({
      dataDir: "/tmp/kosh-fake",
      nowMs: 1_000,
      requestId: "request-1",
    });
    const created = await backend.createTidbit({
      title: null,
      bodyMarkdown: "# Shower thought\n\nKeep the exact body.",
      sources: [
        {
          label: " Reference ",
          url: "HTTPS://Example.COM:443/page#fragment",
        },
      ],
    });

    expect(created.displayTitle).toBe("Shower thought");
    expect(created.sources[0]).toMatchObject({
      label: "Reference",
      url: "https://example.com/page",
    });
    const edited = await backend.editTidbit({
      id: created.id,
      expectedRevisionId: created.currentRevisionId,
      title: "Revised",
      bodyMarkdown: "Updated body",
      sources: [],
    });
    await expect(
      backend.editTidbit({
        id: created.id,
        expectedRevisionId: created.currentRevisionId,
        title: null,
        bodyMarkdown: "Lost update",
        sources: [],
      }),
    ).rejects.toThrow("stale");

    expect((await backend.listTidbits({ limit: 10, cursor: null })).items).toHaveLength(1);
    const deleted = await backend.deleteTidbit({
      id: edited.id,
      expectedRevisionId: edited.currentRevisionId,
    });
    expect(deleted.deletedAtMs).not.toBeNull();
    expect((await backend.listTidbits({ limit: 10, cursor: null })).items).toEqual([]);
    expect((await backend.loadTidbit(deleted.id)).deletedAtMs).toBe(deleted.deletedAtMs);
  });
});
