/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_KOSH_BACKEND?: "fake";
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
