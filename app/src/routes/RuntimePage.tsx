import { useEffect, useState } from "react";
import type { RuntimeProbe } from "../backend/contracts";
import { useBackend } from "../backend/context";

type ProbeState =
  | { status: "loading" }
  | { status: "ready"; probe: RuntimeProbe }
  | { status: "error"; message: string };

export function RuntimePage() {
  const backend = useBackend();
  const [state, setState] = useState<ProbeState>({ status: "loading" });

  useEffect(() => {
    let active = true;
    void backend.runtimeProbe().then(
      (probe) => {
        if (active) setState({ status: "ready", probe });
      },
      (error: unknown) => {
        if (active) {
          setState({
            status: "error",
            message: error instanceof Error ? error.message : String(error),
          });
        }
      },
    );
    return () => {
      active = false;
    };
  }, [backend]);

  return (
    <main className="page">
      <header className="page-header">
        <div>
          <p className="page-kicker">Typed IPC smoke route</p>
          <h1>Runtime</h1>
          <p>This internal view proves the frontend/native boundary.</p>
        </div>
      </header>
      {state.status === "loading" && <p role="status">Loading runtime probe…</p>}
      {state.status === "error" && <p role="alert">{state.message}</p>}
      {state.status === "ready" && (
        <dl aria-label="Runtime probe">
          <dt>Data root</dt>
          <dd>{state.probe.dataDir}</dd>
          <dt>Request</dt>
          <dd>{state.probe.requestId}</dd>
          <dt>Clock</dt>
          <dd>{state.probe.nowMs}</dd>
        </dl>
      )}
    </main>
  );
}
