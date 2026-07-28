import { invoke } from "@tauri-apps/api/core";
import type {
  Backend,
  CitationResolution,
  ClearDraftInput,
  DeleteTidbitInput,
  DraftRecord,
  EditTidbitInput,
  ImageDropIngestResult,
  ImageOcrDiagnostics,
  ImageRecord,
  ImageStatusRecord,
  ListTidbitsInput,
  PassageEmbeddingIndexStatus,
  RuntimeProbe,
  RestoreTidbitInput,
  SaveDraftInput,
  SearchPassagesInput,
  SearchPassagesResponse,
  SemanticRuntimeLogs,
  SemanticRuntimeStatus,
  TidbitDraft,
  TidbitListPage,
  TidbitRecord,
} from "./contracts";

export const tauriBackend: Backend = {
  runtimeProbe: () => invoke<RuntimeProbe>("runtime_probe"),
  semanticRuntimeStatus: () => invoke<SemanticRuntimeStatus>("semantic_runtime_status"),
  prepareSemanticRuntime: () => invoke<SemanticRuntimeStatus>("prepare_semantic_runtime"),
  retrySemanticRuntime: () => invoke<SemanticRuntimeStatus>("retry_semantic_runtime"),
  repairSemanticRuntime: () => invoke<SemanticRuntimeStatus>("repair_semantic_runtime"),
  semanticRuntimeLogs: () => invoke<SemanticRuntimeLogs>("semantic_runtime_logs"),
  passageEmbeddingIndexStatus: () =>
    invoke<PassageEmbeddingIndexStatus>("passage_embedding_index_status"),
  createTidbit: (input: TidbitDraft) => invoke<TidbitRecord>("create_tidbit", { input }),
  loadTidbit: (id: string) => invoke<TidbitRecord>("load_tidbit", { id }),
  listTidbits: (input: ListTidbitsInput) => invoke<TidbitListPage>("list_tidbits", { input }),
  editTidbit: (input: EditTidbitInput) => invoke<TidbitRecord>("edit_tidbit", { input }),
  deleteTidbit: (input: DeleteTidbitInput) => invoke<TidbitRecord>("delete_tidbit", { input }),
  restoreTidbit: (input: RestoreTidbitInput) => invoke<TidbitRecord>("restore_tidbit", { input }),
  resolveCitation: (passageId: string) =>
    invoke<CitationResolution>("resolve_citation", { passageId }),
  searchPassages: (input: SearchPassagesInput) =>
    invoke<SearchPassagesResponse>("search_passages", { input }),
  saveDraft: (input: SaveDraftInput) => invoke<DraftRecord>("save_draft", { input }),
  loadDraft: (contextKey: string) => invoke<DraftRecord | null>("load_draft", { contextKey }),
  clearDraft: (input: ClearDraftInput) => invoke<boolean>("clear_draft", { input }),
  pickImage: (draftId: string) => invoke<ImageRecord | null>("pick_image", { draftId }),
  captureClipboardImage: () => invoke<string>("capture_clipboard_image"),
  ingestClipboardImage: (captureId: string, draftId: string) =>
    invoke<ImageRecord>("ingest_clipboard_image", { captureId, draftId }),
  ingestDroppedImages: (dropId: string, draftId: string) =>
    invoke<ImageDropIngestResult>("ingest_dropped_images", { dropId, draftId }),
  imageStatus: (attachmentId: string) =>
    invoke<ImageStatusRecord>("image_status", { attachmentId }),
  retryImageOcr: (attachmentId: string) =>
    invoke<ImageStatusRecord>("retry_image_ocr", { attachmentId }),
  imageOcrDiagnostics: () => invoke<ImageOcrDiagnostics>("image_ocr_diagnostics"),
};
