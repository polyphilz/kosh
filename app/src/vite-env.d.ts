/// <reference types="vite/client" />

import type { Backend } from "./backend/contracts";

declare global {
  interface Window {
    __KOSH_FAKE_BACKEND__?: Backend;
  }
}

interface ImportMetaEnv {
  readonly VITE_KOSH_BACKEND?: "fake";
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
