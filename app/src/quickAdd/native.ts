import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { TauriCommand, TauriEvent } from "../tauriProtocol";

export interface QuickAddNative {
  dismiss: () => Promise<void>;
  onShown: (listener: () => void) => Promise<() => void>;
  setFileDialogOpen: (open: boolean) => Promise<void>;
}

export const quickAddNative: QuickAddNative = {
  dismiss: () => invoke<void>(TauriCommand.DismissQuickAdd),
  onShown: (listener) => listen(TauriEvent.QuickAddShown, listener),
  setFileDialogOpen: (open) => invoke<void>(TauriCommand.SetQuickAddFileDialogOpen, { open }),
};
