import { describe, expect, it } from "vitest";
import { FakeBackend } from "../../src/backend/fakeBackend";

describe("FakeBackend tidbits", () => {
  it("reports live library diagnostics and deterministic idempotent maintenance", async () => {
    const backend = new FakeBackend();
    await backend.seedNote({
      bodyMarkdown: "A locally searchable fixture.",
      sources: [],
    });

    const diagnostics = await backend.loadMaintenanceDiagnostics();
    expect(diagnostics.library).toMatchObject({
      activeTidbits: 1,
      revisions: 1,
      searchableBlocks: 1,
    });
    expect(diagnostics.storage.dataRoot).toBe("/tmp/kosh-browser-fixture");
    expect(diagnostics.backupPhase).toBe("AVAILABLE");
    await expect(backend.runIntegrityCheck()).resolves.toMatchObject({
      databaseOk: true,
      media: { missingBlobAttachmentIds: [] },
    });
    await expect(backend.rebuildSearchIndexes()).resolves.toMatchObject({
      operation: "REBUILD_SEARCH",
      changedItems: 1,
    });
    await expect(backend.rebuildSearchIndexes()).resolves.toMatchObject({
      operation: "REBUILD_SEARCH",
      changedItems: 1,
    });
  });

  it("mirrors create, optimistic edit, listing, and soft deletion", async () => {
    const backend = new FakeBackend({
      dataDir: "/tmp/kosh-fake",
      nowMs: 1_000,
      requestId: "request-1",
    });
    const created = await backend.seedNote({
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
    const edited = await backend.replaceNoteForTest({
      id: created.id,
      expectedRevisionId: created.currentRevisionId,
      bodyMarkdown: "Updated body",
      sources: [],
    });
    await expect(
      backend.replaceNoteForTest({
        id: created.id,
        expectedRevisionId: created.currentRevisionId,
        bodyMarkdown: "Lost update",
        sources: [],
      }),
    ).rejects.toThrow("stale");

    expect(
      (await backend.listNotesForTest({ limit: 10, cursor: null, scope: "ACTIVE" })).items,
    ).toHaveLength(1);
    const deleted = await backend.deleteTidbit({
      id: edited.id,
      expectedRevisionId: edited.currentRevisionId,
    });
    expect(deleted.deletedAtMs).not.toBeNull();
    expect(
      (await backend.listNotesForTest({ limit: 10, cursor: null, scope: "ACTIVE" })).items,
    ).toEqual([]);
    expect((await backend.loadTidbit(deleted.id)).deletedAtMs).toBe(deleted.deletedAtMs);
  });

  it("rejects sources that become duplicates after normalization", async () => {
    const backend = new FakeBackend();
    const duplicateSources = [
      { label: "Reference", url: "HTTPS://Example.COM:443/page#first" },
      { label: " Reference ", url: "https://example.com/page#second" },
    ];

    await expect(
      backend.seedNote({
        bodyMarkdown: "Duplicate provenance",
        sources: duplicateSources,
      }),
    ).rejects.toThrow("sources must not contain duplicates");

    const created = await backend.seedNote({
      bodyMarkdown: "Valid provenance",
      sources: [],
    });
    await expect(
      backend.replaceNoteForTest({
        id: created.id,
        expectedRevisionId: created.currentRevisionId,
        bodyMarkdown: created.bodyMarkdown,
        sources: duplicateSources,
      }),
    ).rejects.toThrow("sources must not contain duplicates");
  });

  it("allocates generated IDs beyond IDs already present in seeded tidbits", async () => {
    const source = new FakeBackend();
    const seed = await source.seedNote({
      bodyMarkdown: "Keep this seeded record.",
      sources: [],
    });
    const seeded = {
      ...seed,
      id: "fake-tidbit-2",
      currentRevisionId: "fake-revision-7",
    };
    const backend = new FakeBackend(undefined, [seeded]);

    const created = await backend.seedNote({
      bodyMarkdown: "Do not overwrite the seed.",
      sources: [],
    });

    expect(created.id).toBe("fake-tidbit-8");
    expect(await backend.loadTidbit(seeded.id)).toEqual(seeded);
    expect(
      (await backend.listNotesForTest({ limit: 10, cursor: null, scope: "ACTIVE" })).items,
    ).toHaveLength(2);
  });

  it("reuses immutable source IDs across tidbits and edits", async () => {
    const backend = new FakeBackend();
    const first = await backend.seedNote({
      bodyMarkdown: "First use.",
      sources: [{ label: "Docs", url: "https://example.com/reference#first" }],
    });
    const second = await backend.seedNote({
      bodyMarkdown: "Second use.",
      sources: [{ label: " Docs ", url: "HTTPS://EXAMPLE.COM:443/reference#second" }],
    });

    expect(second.sources[0]?.id).toBe(first.sources[0]?.id);

    const edited = await backend.replaceNoteForTest({
      id: second.id,
      expectedRevisionId: second.currentRevisionId,
      bodyMarkdown: "Retained during edit.",
      sources: [{ label: "Docs", url: "https://example.com/reference" }],
    });

    expect(edited.sources[0]?.id).toBe(first.sources[0]?.id);
  });

  it("searches only current blocks through edits, deletion, and restoration", async () => {
    const backend = new FakeBackend();
    const created = await backend.seedNote({
      bodyMarkdown: "Original evidence.",
      sources: [{ label: "Original source", url: "https://example.com/original" }],
    });
    await expect(
      backend.searchBlocks({ query: "Original", mode: "EXACT", limit: 10 }),
    ).resolves.toMatchObject({
      results: [{ noteId: created.id, excerpt: "Original evidence." }],
    });

    const edited = await backend.replaceNoteForTest({
      id: created.id,
      expectedRevisionId: created.currentRevisionId,
      bodyMarkdown: "Replacement evidence.",
      sources: [{ label: "Replacement source", url: null }],
    });
    await expect(
      backend.searchBlocks({ query: "Original", mode: "EXACT", limit: 10 }),
    ).resolves.toMatchObject({ results: [] });
    await expect(
      backend.searchBlocks({ query: "Replacement", mode: "EXACT", limit: 10 }),
    ).resolves.toMatchObject({
      results: [{ noteId: edited.id, excerpt: "Replacement evidence." }],
    });

    const deleted = await backend.deleteTidbit({
      id: edited.id,
      expectedRevisionId: edited.currentRevisionId,
    });
    await expect(
      backend.searchBlocks({ query: "Replacement", mode: "EXACT", limit: 10 }),
    ).resolves.toMatchObject({ results: [] });

    await backend.restoreTidbit({
      id: deleted.id,
      expectedRevisionId: deleted.currentRevisionId,
    });
    await expect(
      backend.searchBlocks({ query: "Replacement", mode: "EXACT", limit: 10 }),
    ).resolves.toMatchObject({
      results: [{ noteId: edited.id }],
    });
  });

  it("returns current block-owned lexical results and safe highlights", async () => {
    const backend = new FakeBackend();
    const created = await backend.seedNote({
      bodyMarkdown: "# Résumé review\n\nA naïve draft mentioned the café outcome.",
      sources: [{ label: "Writing guide", url: "https://example.com/cafe" }],
    });

    const response = await backend.searchBlocks({
      query: "naive cafe",
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
      noteId: created.id,
      blockId: expect.any(String),
      matchedFields: expect.arrayContaining(["BODY"]),
    });
    expect(
      results[0]?.highlights.every(
        (highlight) =>
          Number.isSafeInteger(highlight.startChar) &&
          Number.isSafeInteger(highlight.endChar) &&
          highlight.startChar < highlight.endChar,
      ),
    ).toBe(true);

    const ligature = await backend.seedNote({
      bodyMarkdown: "# ﬁle note\n\nCompatibility characters keep original offsets.",
      sources: [],
    });
    const ligatureResponse = await backend.searchBlocks({
      query: "file",
      mode: "DEFAULT",
      limit: 10,
    });
    const ligatureResults = ligatureResponse.results;
    expect(ligatureResults).toHaveLength(1);
    expect(ligatureResults[0]).toMatchObject({
      noteId: ligature.id,
      blockId: expect.any(String),
      highlights: expect.arrayContaining([{ field: "BODY", startChar: 0, endChar: 3 }]),
    });

    await backend.deleteTidbit({
      id: created.id,
      expectedRevisionId: created.currentRevisionId,
    });
    await expect(
      backend.searchBlocks({ query: "cafe", mode: "DEFAULT", limit: 10 }),
    ).resolves.toMatchObject({ results: [], executionMode: "LEXICAL_ONLY" });
  });
});

describe("FakeBackend offsite recovery", () => {
  const credentials = {
    accessKeyId: "fedcba9876543210fedcba9876543210",
    secretAccessKey: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  };

  it("models opt-in setup, separated status, checkpoints, preview, and a non-mutating drill", async () => {
    const backend = new FakeBackend();
    await expect(backend.loadBackupSettings()).resolves.toMatchObject({
      config: null,
      credentialState: "MISSING",
      relational: { phase: "OFF" },
      checkpoint: { phase: "OFF" },
    });
    await expect(
      backend.testBackupConnection({
        backupSetId: null,
        accountId: "0123456789abcdef0123456789abcdef",
        jurisdiction: "DEFAULT",
        bucket: "kosh-local",
        ...credentials,
      }),
    ).resolves.toMatchObject({ verified: true, cleanupComplete: true });

    const configured = await backend.configureBackup({
      expectedRevision: 0,
      backupSetId: null,
      accountId: "0123456789abcdef0123456789abcdef",
      jurisdiction: "DEFAULT",
      bucket: "kosh-local",
      ...credentials,
    });
    expect(configured).toMatchObject({
      config: { enabled: false, revision: 1, bucket: "kosh-local" },
      credentialState: "STORED",
    });
    expect(JSON.stringify(configured)).not.toContain(credentials.accessKeyId);
    expect(JSON.stringify(configured)).not.toContain(credentials.secretAccessKey);

    const enabled = await backend.setBackupEnabled({ expectedRevision: 1, enabled: true });
    expect(enabled).toMatchObject({
      config: { enabled: true, revision: 2 },
      relational: { phase: "RUNNING" },
      checkpoint: { phase: "IDLE" },
    });
    await backend.backupNow();
    const [checkpoint] = await backend.listBackupCheckpoints();
    expect(checkpoint).toBeDefined();
    await expect(
      backend.previewBackupRestore({ checkpointId: checkpoint!.checkpointId }),
    ).resolves.toMatchObject({
      checkpoint,
      owner: { isCurrentInstallation: true },
    });
    await expect(
      backend.drillBackupRestore({ checkpointId: checkpoint!.checkpointId }),
    ).resolves.toMatchObject({
      checkpointId: checkpoint!.checkpointId,
      restoredMediaCount: 0,
    });
  });

  it("rejects partial credentials, stale revisions, active takeover, and stale owner evidence", async () => {
    const backend = new FakeBackend();
    await expect(
      backend.configureBackup({
        expectedRevision: 0,
        backupSetId: null,
        accountId: "0123456789abcdef0123456789abcdef",
        jurisdiction: "DEFAULT",
        bucket: "kosh-local",
        accessKeyId: credentials.accessKeyId,
        secretAccessKey: null,
      }),
    ).rejects.toThrow("both");

    const configured = await backend.configureBackup({
      expectedRevision: 0,
      backupSetId: null,
      accountId: "0123456789abcdef0123456789abcdef",
      jurisdiction: "DEFAULT",
      bucket: "kosh-local",
      ...credentials,
    });
    await expect(backend.setBackupEnabled({ expectedRevision: 0, enabled: true })).rejects.toThrow(
      "changed",
    );

    const enabled = await backend.setBackupEnabled({
      expectedRevision: configured.config!.revision,
      enabled: true,
    });
    const takeover = {
      expectedRevision: enabled.config!.revision,
      expectedOwnerBackupSetId: enabled.config!.backupSetId,
      expectedOwnerReplicaEpochId: enabled.config!.replicaEpochId,
      expectedOwnerWriterId: "fixture-current-installation-writer",
      expectedOwnerVersion: '"fixture-owner-v1"',
      confirmation: "TAKE OVER" as const,
    };
    await expect(backend.takeOverBackup(takeover)).rejects.toThrow("Turn off");

    const disabled = await backend.setBackupEnabled({
      expectedRevision: enabled.config!.revision,
      enabled: false,
    });
    await expect(
      backend.takeOverBackup({
        ...takeover,
        expectedRevision: disabled.config!.revision,
        expectedOwnerVersion: '"stale-owner"',
      }),
    ).rejects.toThrow("owner changed");
    await expect(
      backend.takeOverBackup({
        ...takeover,
        expectedRevision: disabled.config!.revision,
      }),
    ).resolves.toMatchObject({
      config: {
        revision: disabled.config!.revision + 1,
        enabled: false,
        replicaEpochId: "019f547b-6200-7000-8000-000000000e02",
      },
    });
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
    await backend.seedNote({
      bodyMarkdown: "# Prepared runtime\n\nThe fake search implementation remains lexical.",
      sources: [],
    });
    await expect(
      backend.searchBlocks({ query: "lexical", mode: "DEFAULT", limit: 10 }),
    ).resolves.toMatchObject({
      results: [{ displayTitle: "Prepared runtime" }],
      executionMode: "LEXICAL_ONLY",
      semanticReadiness: "READY",
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
