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

  it("rejects sources that become duplicates after normalization", async () => {
    const backend = new FakeBackend();
    const duplicateSources = [
      { label: "Reference", url: "HTTPS://Example.COM:443/page#first" },
      { label: " Reference ", url: "https://example.com/page#second" },
    ];

    await expect(
      backend.createTidbit({
        title: null,
        bodyMarkdown: "Duplicate provenance",
        sources: duplicateSources,
      }),
    ).rejects.toThrow("sources must not contain duplicates");

    const created = await backend.createTidbit({
      title: null,
      bodyMarkdown: "Valid provenance",
      sources: [],
    });
    await expect(
      backend.editTidbit({
        id: created.id,
        expectedRevisionId: created.currentRevisionId,
        title: null,
        bodyMarkdown: created.bodyMarkdown,
        sources: duplicateSources,
      }),
    ).rejects.toThrow("sources must not contain duplicates");
  });

  it("allocates generated IDs beyond IDs already present in seeded tidbits", async () => {
    const source = new FakeBackend();
    const seed = await source.createTidbit({
      title: "Seed",
      bodyMarkdown: "Keep this seeded record.",
      sources: [],
    });
    const seeded = {
      ...seed,
      id: "fake-tidbit-2",
      currentRevisionId: "fake-revision-7",
    };
    const backend = new FakeBackend(undefined, [seeded]);

    const created = await backend.createTidbit({
      title: "New",
      bodyMarkdown: "Do not overwrite the seed.",
      sources: [],
    });

    expect(created.id).toBe("fake-tidbit-8");
    expect(await backend.loadTidbit(seeded.id)).toEqual(seeded);
    expect((await backend.listTidbits({ limit: 10, cursor: null })).items).toHaveLength(2);
  });

  it("reuses immutable source IDs across tidbits and edits", async () => {
    const backend = new FakeBackend();
    const first = await backend.createTidbit({
      title: "First",
      bodyMarkdown: "First use.",
      sources: [{ label: "Docs", url: "https://example.com/reference#first" }],
    });
    const second = await backend.createTidbit({
      title: "Second",
      bodyMarkdown: "Second use.",
      sources: [{ label: " Docs ", url: "HTTPS://EXAMPLE.COM:443/reference#second" }],
    });

    expect(second.sources[0]?.id).toBe(first.sources[0]?.id);

    const edited = await backend.editTidbit({
      id: second.id,
      expectedRevisionId: second.currentRevisionId,
      title: second.title,
      bodyMarkdown: "Retained during edit.",
      sources: [{ label: "Docs", url: "https://example.com/reference" }],
    });

    expect(edited.sources[0]?.id).toBe(first.sources[0]?.id);
  });
});

describe("FakeBackend drafts", () => {
  it("restores exact partial input and protects newer autosaves from stale clears", async () => {
    const backend = new FakeBackend({
      dataDir: "/tmp/kosh-fake",
      nowMs: 1_000,
      requestId: "request-1",
    });
    const first = await backend.saveDraft({
      contextKey: "capture",
      tidbitId: null,
      baseRevisionId: null,
      title: "  unfinished  ",
      bodyMarkdown: "",
      sources: [{ label: null, url: "" }],
    });
    const second = await backend.saveDraft({
      contextKey: "capture",
      tidbitId: null,
      baseRevisionId: null,
      title: null,
      bodyMarkdown: "newer",
      sources: [],
    });

    expect(await backend.loadDraft("capture")).toEqual(second);
    expect(second.id).toBe(first.id);
    expect(second.updatedAtMs).toBeGreaterThan(first.updatedAtMs);
    await expect(
      backend.clearDraft({
        contextKey: "capture",
        expectedUpdatedAtMs: first.updatedAtMs,
      }),
    ).resolves.toBe(false);
    await expect(
      backend.clearDraft({
        contextKey: "capture",
        expectedUpdatedAtMs: second.updatedAtMs,
      }),
    ).resolves.toBe(true);
    await expect(backend.loadDraft("capture")).resolves.toBeNull();
  });

  it("pins edit drafts to a revision owned by that tidbit", async () => {
    const backend = new FakeBackend();
    const tidbit = await backend.createTidbit({
      title: null,
      bodyMarkdown: "Original",
      sources: [],
    });

    await expect(
      backend.saveDraft({
        contextKey: `edit:${tidbit.id}`,
        tidbitId: tidbit.id,
        baseRevisionId: tidbit.currentRevisionId,
        title: null,
        bodyMarkdown: "Editing",
        sources: [],
      }),
    ).resolves.toMatchObject({
      tidbitId: tidbit.id,
      baseRevisionId: tidbit.currentRevisionId,
    });
    await expect(
      backend.saveDraft({
        contextKey: `edit:${tidbit.id}`,
        tidbitId: tidbit.id,
        baseRevisionId: "another-revision",
        title: null,
        bodyMarkdown: "Invalid",
        sources: [],
      }),
    ).rejects.toThrow("must belong");
  });
});
