import type { Backend } from "./contracts";
import { FakeBackend } from "./fakeBackend";
import { tauriBackend } from "./tauriBackend";

export function createBackend(): Backend {
  return import.meta.env.VITE_KOSH_BACKEND === "fake" ? new FakeBackend() : tauriBackend;
}
