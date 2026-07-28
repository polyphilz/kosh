import { invoke } from "@tauri-apps/api/core";
import type {
  Backend,
  CitationResolution,
  ClearDraftInput,
  DeleteTidbitInput,
  DraftRecord,
  EditTidbitInput,
  ListTidbitsInput,
  RuntimeProbe,
  RestoreTidbitInput,
  SaveDraftInput,
  TidbitDraft,
  TidbitListPage,
  TidbitRecord,
} from "./contracts";

export const tauriBackend: Backend = {
  runtimeProbe: () => invoke<RuntimeProbe>("runtime_probe"),
  createTidbit: (input: TidbitDraft) => invoke<TidbitRecord>("create_tidbit", { input }),
  loadTidbit: (id: string) => invoke<TidbitRecord>("load_tidbit", { id }),
  listTidbits: (input: ListTidbitsInput) => invoke<TidbitListPage>("list_tidbits", { input }),
  editTidbit: (input: EditTidbitInput) => invoke<TidbitRecord>("edit_tidbit", { input }),
  deleteTidbit: (input: DeleteTidbitInput) => invoke<TidbitRecord>("delete_tidbit", { input }),
  restoreTidbit: (input: RestoreTidbitInput) => invoke<TidbitRecord>("restore_tidbit", { input }),
  resolveCitation: (passageId: string) =>
    invoke<CitationResolution>("resolve_citation", { passageId }),
  saveDraft: (input: SaveDraftInput) => invoke<DraftRecord>("save_draft", { input }),
  loadDraft: (contextKey: string) => invoke<DraftRecord | null>("load_draft", { contextKey }),
  clearDraft: (input: ClearDraftInput) => invoke<boolean>("clear_draft", { input }),
};
