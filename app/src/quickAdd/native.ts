import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { TauriCommand, TauriEvent } from "../tauriProtocol";

export const QuickAddDismissAction = {
  Back: "BACK",
  CheckForUpdates: "CHECK_FOR_UPDATES",
  Dismiss: "DISMISS",
  DismissPreserveFocus: "DISMISS_PRESERVE_FOCUS",
  Forward: "FORWARD",
  NewNote: "NEW_NOTE",
  Search: "SEARCH",
  Settings: "SETTINGS",
  ShowMain: "SHOW_MAIN",
  ToggleSidebar: "TOGGLE_SIDEBAR",
} as const;

export type QuickAddDismissAction =
  (typeof QuickAddDismissAction)[keyof typeof QuickAddDismissAction];

export interface QuickAddDismissRequest {
  action: QuickAddDismissAction;
}

export interface QuickAddNative {
  cancelDismiss: () => Promise<void>;
  dismiss: (action: QuickAddDismissAction) => Promise<void>;
  onDismissRequested: (listener: (request: QuickAddDismissRequest) => void) => Promise<() => void>;
  onShown: (listener: () => void) => Promise<() => void>;
  setFileDialogOpen: (open: boolean) => Promise<void>;
}

export const quickAddNative: QuickAddNative = {
  cancelDismiss: () => invoke<void>(TauriCommand.CancelQuickAddDismiss),
  dismiss: (action) => invoke<void>(TauriCommand.CompleteQuickAddDismiss, { action }),
  onDismissRequested: (listener) =>
    listen<QuickAddDismissRequest>(TauriEvent.QuickAddDismissRequested, (event) =>
      listener(event.payload),
    ),
  onShown: (listener) => listen(TauriEvent.QuickAddShown, listener),
  setFileDialogOpen: (open) => invoke<void>(TauriCommand.SetQuickAddFileDialogOpen, { open }),
};
