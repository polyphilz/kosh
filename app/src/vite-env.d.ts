/// <reference types="vite/client" />

import type { FakeBackend } from "./backend/fakeBackend";

declare global {
  interface Window {
    __KOSH_FAKE_BACKEND__?: FakeBackend;
  }
}

interface ImportMetaEnv {
  readonly VITE_KOSH_BACKEND?: "fake";
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
