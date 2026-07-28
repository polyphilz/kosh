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

  it("keeps revision-bound citations historical through edits and deletion", async () => {
    const backend = new FakeBackend();
    const created = await backend.createTidbit({
      title: "First",
      bodyMarkdown: "Original evidence.",
      sources: [{ label: "Original source", url: "https://example.com/original" }],
    });
    const originalPassageId = `fake-passage:${created.currentRevisionId}`;
    await expect(backend.resolveCitation(originalPassageId)).resolves.toMatchObject({
      state: "CURRENT",
      excerpt: "Original evidence.",
      tidbit: { revisionId: created.currentRevisionId, deleted: false },
      sources: [{ label: "Original source" }],
    });

    const edited = await backend.editTidbit({
      id: created.id,
      expectedRevisionId: created.currentRevisionId,
      title: "Second",
      bodyMarkdown: "Replacement evidence.",
      sources: [{ label: "Replacement source", url: null }],
    });
    const replacementPassageId = `fake-passage:${edited.currentRevisionId}`;
    await expect(backend.resolveCitation(originalPassageId)).resolves.toMatchObject({
      state: "HISTORICAL",
      excerpt: "Original evidence.",
      tidbit: { revisionId: created.currentRevisionId, deleted: false },
      sources: [{ label: "Original source" }],
    });
    await expect(backend.resolveCitation(replacementPassageId)).resolves.toMatchObject({
      state: "CURRENT",
      excerpt: "Replacement evidence.",
      sources: [{ label: "Replacement source" }],
    });

    const deleted = await backend.deleteTidbit({
      id: edited.id,
      expectedRevisionId: edited.currentRevisionId,
    });
    await expect(backend.resolveCitation(replacementPassageId)).resolves.toMatchObject({
      state: "HISTORICAL",
      tidbit: { deleted: true },
    });

    await backend.restoreTidbit({
      id: deleted.id,
      expectedRevisionId: deleted.currentRevisionId,
    });
    await expect(backend.resolveCitation(replacementPassageId)).resolves.toMatchObject({
      state: "CURRENT",
      tidbit: { deleted: false },
    });
    await expect(backend.resolveCitation(originalPassageId)).resolves.toMatchObject({
      state: "HISTORICAL",
    });
  });

  it("returns current citation-owned lexical results and safe highlights", async () => {
    const backend = new FakeBackend();
    const created = await backend.createTidbit({
      title: "Résumé review",
      bodyMarkdown: "A naïve draft mentioned the café outcome.",
      sources: [{ label: "Writing guide", url: "https://example.com/cafe" }],
    });

    const response = await backend.searchPassages({
      query: "resume cafe",
      mode: "EXACT",
      limit: 10,
    });
    expect(response).toMatchObject({
      executionMode: "EXACT",
      semanticReadiness: "NOT_REQUESTED",
    });
    const { results } = response;
    expect(results).toHaveLength(1);
    expect(results[0]).toMatchObject({
      passageId: `fake-passage:${created.currentRevisionId}`,
      matchedFields: expect.arrayContaining(["TITLE", "BODY"]),
      citation: {
        state: "CURRENT",
        tidbit: { revisionId: created.currentRevisionId },
      },
    });
    expect(
      results[0]?.highlights.every(
        (highlight) =>
          Number.isSafeInteger(highlight.startChar) &&
          Number.isSafeInteger(highlight.endChar) &&
          highlight.startChar < highlight.endChar,
      ),
    ).toBe(true);

    const ligature = await backend.createTidbit({
      title: "ﬁle note",
      bodyMarkdown: "Compatibility characters keep original offsets.",
      sources: [],
    });
    const ligatureResponse = await backend.searchPassages({
      query: "file",
      mode: "DEFAULT",
      limit: 10,
    });
    const ligatureResults = ligatureResponse.results;
    expect(ligatureResults).toHaveLength(1);
    expect(ligatureResults[0]).toMatchObject({
      passageId: `fake-passage:${ligature.currentRevisionId}`,
      highlights: expect.arrayContaining([{ field: "TITLE", startChar: 0, endChar: 3 }]),
    });

    await backend.deleteTidbit({
      id: created.id,
      expectedRevisionId: created.currentRevisionId,
    });
    await expect(
      backend.searchPassages({ query: "cafe", mode: "DEFAULT", limit: 10 }),
    ).resolves.toMatchObject({ results: [], executionMode: "LEXICAL_ONLY" });
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

describe("FakeBackend semantic runtime", () => {
  it("exposes deterministic prepare, retry, repair, and log states", async () => {
    const backend = new FakeBackend();

    await expect(backend.semanticRuntimeStatus()).resolves.toMatchObject({
      phase: "NOT_DOWNLOADED",
      verified: false,
      runtimeRunning: false,
    });
    await expect(backend.prepareSemanticRuntime()).resolves.toMatchObject({
      phase: "READY",
      verified: true,
      runtimeRunning: true,
    });
    await expect(backend.retrySemanticRuntime()).resolves.toMatchObject({
      phase: "READY",
    });
    await expect(backend.repairSemanticRuntime()).resolves.toMatchObject({
      phase: "READY",
      downloadedBytes: 232_883_776,
    });
    await expect(backend.semanticRuntimeLogs()).resolves.toEqual({
      text: "",
      truncated: false,
    });
  });
});
