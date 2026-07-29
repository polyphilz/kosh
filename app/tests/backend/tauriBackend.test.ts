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
  SetShortcutSettingsInput,
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
    const shortcuts: SetShortcutSettingsInput = {
      expectedRevision: 1,
      keyboardBindings: [
        { command: "QUICK_ADD", accelerator: "control+alt+super+KeyK" },
        { command: "MAIN_WINDOW", accelerator: "control+alt+super+KeyO" },
      ],
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
    await tauriBackend.loadShortcutSettings();
    await tauriBackend.setShortcutSettings(shortcuts);
    await tauriBackend.selectImage();
    await tauriBackend.ingestSelectedImage("selection-1", "draft-1");
    await tauriBackend.captureClipboardImage();
    await tauriBackend.ingestClipboardImage("capture-1", "draft-1");
    await tauriBackend.ingestDroppedImages("drop-1", "draft-1");
    await tauriBackend.imageStatus("attachment-1");
    await tauriBackend.retryImageOcr("attachment-1");
    await tauriBackend.imageOcrDiagnostics();
    await tauriBackend.selectPdf();
    await tauriBackend.ingestSelectedPdf("pdf-selection-1", "draft-1");
    await tauriBackend.selectAttachment();
    await tauriBackend.ingestSelectedAttachment("file-selection-1", "draft-1");
    await tauriBackend.attachmentStatus("file-attachment-1");
    await tauriBackend.openAttachmentExternal("file-attachment-1");
    await tauriBackend.revealAttachmentInFinder("file-attachment-1");
    await tauriBackend.setFileDropConsumerActive(true);
    await tauriBackend.discardFileDropSelections(["pdf-selection-2"]);
    await tauriBackend.pdfStatus("pdf-attachment-1");
    await tauriBackend.retryPdfExtraction("pdf-attachment-1");
    await tauriBackend.openPdfExternal("pdf-attachment-1");

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
      ["load_shortcut_settings"],
      ["set_shortcut_settings", { input: shortcuts }],
      ["select_image"],
      ["ingest_selected_image", { selectionId: "selection-1", draftId: "draft-1" }],
      ["capture_clipboard_image"],
      ["ingest_clipboard_image", { captureId: "capture-1", draftId: "draft-1" }],
      ["ingest_dropped_images", { dropId: "drop-1", draftId: "draft-1" }],
      ["image_status", { attachmentId: "attachment-1" }],
      ["retry_image_ocr", { attachmentId: "attachment-1" }],
      ["image_ocr_diagnostics"],
      ["select_pdf"],
      ["ingest_selected_pdf", { selectionId: "pdf-selection-1", draftId: "draft-1" }],
      ["select_attachment"],
      ["ingest_selected_attachment", { selectionId: "file-selection-1", draftId: "draft-1" }],
      ["attachment_status", { attachmentId: "file-attachment-1" }],
      ["open_attachment_external", { attachmentId: "file-attachment-1" }],
      ["reveal_attachment_in_finder", { attachmentId: "file-attachment-1" }],
      ["set_file_drop_consumer_active", { active: true }],
      ["discard_file_drop_selections", { selectionIds: ["pdf-selection-2"] }],
      ["pdf_status", { attachmentId: "pdf-attachment-1" }],
      ["retry_pdf_extraction", { attachmentId: "pdf-attachment-1" }],
      ["open_pdf_external", { attachmentId: "pdf-attachment-1" }],
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
