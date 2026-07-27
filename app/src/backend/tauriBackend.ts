import { invoke } from "@tauri-apps/api/core";
import type {
  Backend,
  DeleteTidbitInput,
  EditTidbitInput,
  ListTidbitsInput,
  RuntimeProbe,
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
};
