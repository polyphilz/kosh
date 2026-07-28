import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ClearDraftInput,
  DeleteTidbitInput,
  EditTidbitInput,
  ListTidbitsInput,
  RestoreTidbitInput,
  SaveDraftInput,
  SearchPassagesInput,
  TidbitDraft,
} from "../../src/backend/contracts";
import { tauriBackend } from "../../src/backend/tauriBackend";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

describe("tauriBackend tidbit gateway", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("uses typed command names and payload envelopes", async () => {
    vi.mocked(invoke).mockResolvedValue({});
    const draft: TidbitDraft = {
      title: null,
      bodyMarkdown: "A shower thought",
      sources: [{ label: "Notebook", url: null }],
    };
    const edit: EditTidbitInput = {
      ...draft,
      id: "tidbit-1",
      expectedRevisionId: "revision-1",
    };
    const deletion: DeleteTidbitInput = {
      id: "tidbit-1",
      expectedRevisionId: "revision-2",
    };
    const restoration: RestoreTidbitInput = { ...deletion };
    const list: ListTidbitsInput = {
      limit: 50,
      cursor: { updatedAtMs: 42, id: "tidbit-1" },
    };
    const savedDraft: SaveDraftInput = {
      contextKey: "capture",
      tidbitId: null,
      baseRevisionId: null,
      ...draft,
    };
    const clearDraft: ClearDraftInput = {
      contextKey: "capture",
      expectedUpdatedAtMs: 42,
    };
    const search: SearchPassagesInput = {
      query: '"exact phrase"',
      mode: "EXACT",
      limit: 20,
    };

    await tauriBackend.createTidbit(draft);
    await tauriBackend.loadTidbit("tidbit-1");
    await tauriBackend.listTidbits(list);
    await tauriBackend.editTidbit(edit);
    await tauriBackend.deleteTidbit(deletion);
    await tauriBackend.restoreTidbit(restoration);
    await tauriBackend.resolveCitation("passage-1");
    await tauriBackend.searchPassages(search);
    await tauriBackend.saveDraft(savedDraft);
    await tauriBackend.loadDraft("capture");
    await tauriBackend.clearDraft(clearDraft);

    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["create_tidbit", { input: draft }],
      ["load_tidbit", { id: "tidbit-1" }],
      ["list_tidbits", { input: list }],
      ["edit_tidbit", { input: edit }],
      ["delete_tidbit", { input: deletion }],
      ["restore_tidbit", { input: restoration }],
      ["resolve_citation", { passageId: "passage-1" }],
      ["search_passages", { input: search }],
      ["save_draft", { input: savedDraft }],
      ["load_draft", { contextKey: "capture" }],
      ["clear_draft", { input: clearDraft }],
    ]);
  });

  it("uses explicit semantic runtime lifecycle commands", async () => {
    vi.mocked(invoke).mockResolvedValue({});

    await tauriBackend.semanticRuntimeStatus();
    await tauriBackend.prepareSemanticRuntime();
    await tauriBackend.retrySemanticRuntime();
    await tauriBackend.repairSemanticRuntime();
    await tauriBackend.semanticRuntimeLogs();

    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["semantic_runtime_status"],
      ["prepare_semantic_runtime"],
      ["retry_semantic_runtime"],
      ["repair_semantic_runtime"],
      ["semantic_runtime_logs"],
    ]);
  });
});
