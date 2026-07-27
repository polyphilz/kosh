import type { Backend, RuntimeProbe } from "./contracts";

export const browserRuntimeProbe: RuntimeProbe = {
  dataDir: "/tmp/kosh-browser-fixture",
  nowMs: 1_785_201_600_000,
  requestId: "fixture-request-1",
};

export class FakeBackend implements Backend {
  private readonly probe: RuntimeProbe;

  constructor(probe: RuntimeProbe = browserRuntimeProbe) {
    this.probe = probe;
  }

  async runtimeProbe(): Promise<RuntimeProbe> {
    return { ...this.probe };
  }
}
