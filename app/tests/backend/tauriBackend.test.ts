import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  DeleteTidbitInput,
  EditTidbitInput,
  ListTidbitsInput,
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
    const list: ListTidbitsInput = {
      limit: 50,
      cursor: { updatedAtMs: 42, id: "tidbit-1" },
    };

    await tauriBackend.createTidbit(draft);
    await tauriBackend.loadTidbit("tidbit-1");
    await tauriBackend.listTidbits(list);
    await tauriBackend.editTidbit(edit);
    await tauriBackend.deleteTidbit(deletion);

    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["create_tidbit", { input: draft }],
      ["load_tidbit", { id: "tidbit-1" }],
      ["list_tidbits", { input: list }],
      ["edit_tidbit", { input: edit }],
      ["delete_tidbit", { input: deletion }],
    ]);
  });
});
