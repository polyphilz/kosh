import { invoke } from "@tauri-apps/api/core";
import type { Backend, RuntimeProbe } from "./contracts";

export const tauriBackend: Backend = {
  runtimeProbe: () => invoke<RuntimeProbe>("runtime_probe"),
};
