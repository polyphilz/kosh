import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  CheckpointWorkingCopyInput,
  ConfigureBackupInput,
  DeleteTidbitInput,
  DiscardWorkingCopyInput,
  RestoreTidbitInput,
  SaveWorkingCopyInput,
  SearchPassagesInput,
  SetAutomaticUpdateChecksInput,
  SetShortcutSettingsInput,
  TakeOverBackupInput,
  TestBackupConnectionInput,
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
    const workingCopy: SaveWorkingCopyInput = {
      noteId: "tidbit-1",
      baseRevisionId: null,
      editGeneration: 1,
      bodyMarkdown: "A shower thought",
      sources: [{ label: "Notebook", url: null }],
    };
    const checkpoint: CheckpointWorkingCopyInput = {
      noteId: "tidbit-1",
      expectedEditGeneration: 1,
    };
    const discard: DiscardWorkingCopyInput = { ...checkpoint };
    const deletion: DeleteTidbitInput = {
      id: "tidbit-1",
      expectedRevisionId: "revision-2",
    };
    const restoration: RestoreTidbitInput = { ...deletion };
    const search: SearchPassagesInput = {
      query: '"exact phrase"',
      mode: "EXACT",
      limit: 20,
    };
    const shortcuts: SetShortcutSettingsInput = {
      expectedRevision: 1,
      keyboardBindings: [{ command: "MAIN_WINDOW", accelerator: "control+alt+super+KeyO" }],
    };
    const automaticUpdates: SetAutomaticUpdateChecksInput = {
      enabled: false,
      expectedRevision: 1,
    };

    await tauriBackend.copyText("http://tauri.localhost/#/notes/tidbit-1");
    await tauriBackend.loadTidbit("tidbit-1");
    await tauriBackend.deleteTidbit(deletion);
    await tauriBackend.restoreTidbit(restoration);
    await tauriBackend.openSourceUrl("source-1");
    await tauriBackend.resolveCitation("passage-1");
    await tauriBackend.searchPassages(search);
    await tauriBackend.saveWorkingCopy(workingCopy);
    await tauriBackend.reserveWorkingCopyForMedia(workingCopy);
    await tauriBackend.loadWorkingCopy("tidbit-1");
    await tauriBackend.listWorkingCopies();
    await tauriBackend.checkpointWorkingCopy(checkpoint);
    await tauriBackend.discardWorkingCopy(discard);
    await tauriBackend.loadShortcutSettings();
    await tauriBackend.setAutomaticUpdateChecks(automaticUpdates);
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
      ["copy_text", { text: "http://tauri.localhost/#/notes/tidbit-1" }],
      ["load_tidbit", { id: "tidbit-1" }],
      ["delete_tidbit", { input: deletion }],
      ["restore_tidbit", { input: restoration }],
      ["open_source_url", { sourceId: "source-1" }],
      ["resolve_citation", { passageId: "passage-1" }],
      ["search_passages", { input: search }],
      ["save_working_copy", { input: workingCopy }],
      ["reserve_working_copy_for_media", { input: workingCopy }],
      ["load_working_copy", { noteId: "tidbit-1" }],
      ["list_working_copies"],
      ["checkpoint_working_copy", { input: checkpoint }],
      ["discard_working_copy", { input: discard }],
      ["load_shortcut_settings"],
      ["set_automatic_update_checks", { input: automaticUpdates }],
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

  it("uses explicit write-only backup and recovery command envelopes", async () => {
    vi.mocked(invoke).mockResolvedValue({});
    const target: TestBackupConnectionInput = {
      backupSetId: null,
      accountId: "0123456789abcdef0123456789abcdef",
      jurisdiction: "DEFAULT",
      bucket: "kosh-local",
      accessKeyId: "0123456789abcdef0123456789abcdef",
      secretAccessKey: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    };
    const configure: ConfigureBackupInput = {
      expectedRevision: 0,
      ...target,
    };
    const checkpoint = { checkpointId: "019f547b-6200-7000-8000-000000000001" };
    const takeover: TakeOverBackupInput = {
      expectedRevision: 2,
      expectedOwnerBackupSetId: "019f547b-6200-7000-8000-000000000002",
      expectedOwnerReplicaEpochId: "019f547b-6200-7000-8000-000000000003",
      expectedOwnerWriterId: "a".repeat(64),
      expectedOwnerVersion: '"etag"',
      confirmation: "TAKE OVER",
    };

    await tauriBackend.loadBackupSettings();
    await tauriBackend.testBackupConnection(target);
    await tauriBackend.configureBackup(configure);
    await tauriBackend.setBackupEnabled({ expectedRevision: 1, enabled: true });
    await tauriBackend.backupNow();
    await tauriBackend.listBackupCheckpoints();
    await tauriBackend.previewBackupRestore(checkpoint);
    await tauriBackend.drillBackupRestore(checkpoint);
    await tauriBackend.takeOverBackup(takeover);

    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["load_backup_settings"],
      ["test_backup_connection", { input: target }],
      ["configure_backup", { input: configure }],
      ["set_backup_enabled", { input: { expectedRevision: 1, enabled: true } }],
      ["backup_now"],
      ["list_backup_checkpoints"],
      ["preview_backup_restore", { input: checkpoint }],
      ["drill_backup_restore", { input: checkpoint }],
      ["take_over_backup", { input: takeover }],
    ]);
  });

  it("uses explicit diagnostics and maintenance commands", async () => {
    vi.mocked(invoke).mockResolvedValue({});

    await tauriBackend.loadMaintenanceDiagnostics();
    await tauriBackend.runIntegrityCheck();
    await tauriBackend.rebuildSearchIndexes();
    await tauriBackend.rebuildEmbeddingIndex();
    await tauriBackend.retryFailedExtractions();
    await tauriBackend.reclaimEligibleMedia();

    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["load_maintenance_diagnostics"],
      ["run_integrity_check"],
      ["rebuild_search_indexes"],
      ["rebuild_embedding_index"],
      ["retry_failed_extractions"],
      ["reclaim_eligible_media"],
    ]);
  });
});
