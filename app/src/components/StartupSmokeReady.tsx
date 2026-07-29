import { emit } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { useBackend } from "../backend/context";

interface StartupSmokeReadyProps {
  surface: "main" | "quick-add";
}

export function StartupSmokeReady({ surface }: StartupSmokeReadyProps) {
  const backend = useBackend();

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;

    let canceled = false;
    void (async () => {
      const probe = await backend.runtimeProbe();
      if (canceled) return;
      const root = document.getElementById("root");
      await emit("kosh://startup-smoke-ready", {
        surface,
        rendered: Boolean(root?.firstElementChild),
        documentReadyState: document.readyState,
        rootChildCount: root?.childElementCount ?? 0,
        frontendOrigin: window.location.origin,
        probeDataDir: probe.dataDir,
        probeRequestId: probe.requestId,
      });
    })().catch((error: unknown) => {
      console.error("Kosh startup readiness probe failed", error);
    });
    return () => {
      canceled = true;
    };
  }, [backend, surface]);

  return null;
}
