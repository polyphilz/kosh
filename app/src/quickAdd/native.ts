import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const QUICK_ADD_SHOWN_EVENT = "kosh://quick-add-shown";

export interface QuickAddNative {
  dismiss: () => Promise<void>;
  onShown: (listener: () => void) => Promise<() => void>;
  setFileDialogOpen: (open: boolean) => Promise<void>;
}

export const quickAddNative: QuickAddNative = {
  dismiss: () => invoke<void>("dismiss_quick_add"),
  onShown: (listener) => listen(QUICK_ADD_SHOWN_EVENT, listener),
  setFileDialogOpen: (open) => invoke<void>("set_quick_add_file_dialog_open", { open }),
};
