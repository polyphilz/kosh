import type { Backend } from "./contracts";
import { FakeBackend } from "./fakeBackend";
import { tauriBackend } from "./tauriBackend";

export function createBackend(): Backend {
  if (import.meta.env.VITE_KOSH_BACKEND !== "fake") {
    return tauriBackend;
  }
  const backend = new FakeBackend();
  Object.defineProperty(window, "__KOSH_FAKE_BACKEND__", {
    configurable: true,
    value: backend,
  });
  return backend;
}
