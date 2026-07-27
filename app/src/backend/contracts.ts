export interface RuntimeProbe {
  dataDir: string;
  nowMs: number;
  requestId: string;
}

export interface Backend {
  runtimeProbe(): Promise<RuntimeProbe>;
}
