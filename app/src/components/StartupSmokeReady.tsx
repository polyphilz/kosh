import { emit } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { useBackend } from "../backend/context";
import type { Backend } from "../backend/contracts";
import { createUuidV7, NoteAutosaveCoordinator } from "../notes/autosave";
import { TauriEvent } from "../tauriProtocol";

interface StartupSmokeReadyProps {
  surface: "main";
}

const CANARY_URL = "https://example.invalid/kosh-progressive-operability";
let startupCapture: Promise<boolean> | undefined;

interface StartupCanaryEvidence {
  citationState: "CURRENT" | "HISTORICAL";
  executionMode: "EXACT" | "HYBRID" | "LEXICAL_ONLY";
  passageId: string;
  resolvedPassageId: string;
  revisionId: string;
  resultCount: number;
  canaryUrl: string;
}

export function StartupSmokeReady({ surface }: StartupSmokeReadyProps) {
  const backend = useBackend();

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;

    let canceled = false;
    void (async () => {
      const probe = await backend.runtimeProbe();
      if (!probe.startupSmokeCanary) return;
      const captureCreated =
        probe.startupSmokeCapture && surface === "main"
          ? await (startupCapture ??= captureCanary(backend, probe.startupSmokeCanary))
          : false;
      if (canceled) return;
      const canary = await proveSearchAndCitation(backend, probe.startupSmokeCanary);
      if (canceled) return;
      const root = document.getElementById("root");
      await emit(TauriEvent.StartupSmokeReady, {
        surface,
        rendered: Boolean(root?.firstElementChild),
        captureCreated,
        documentReadyState: document.readyState,
        rootChildCount: root?.childElementCount ?? 0,
        frontendOrigin: window.location.origin,
        probeDataDir: probe.dataDir,
        probeRequestId: probe.requestId,
        canary,
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

async function captureCanary(backend: Backend, query: string): Promise<boolean> {
  const existing = await backend.searchPassages({
    query,
    mode: "EXACT",
    limit: 10,
  });
  if (existing.results.some(({ citation }) => citation.excerpt.includes(query))) return false;
  const coordinator = NoteAutosaveCoordinator.ephemeral(backend, { noteId: createUuidV7() });
  coordinator.update(`${query}\n\n${CANARY_URL}`);
  await coordinator.flush("IDLE");
  coordinator.dispose();
  return true;
}

async function proveSearchAndCitation(
  backend: Backend,
  query: string,
): Promise<StartupCanaryEvidence> {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    const response = await backend.searchPassages({
      query,
      mode: "EXACT",
      limit: 10,
    });
    const matches = response.results.filter(({ citation }) => citation.excerpt.includes(query));
    if (matches.length > 1) {
      throw new Error(`startup canary search returned ${matches.length} matching passages`);
    }
    if (matches.length === 0) {
      await new Promise((resolve) => setTimeout(resolve, 100));
      continue;
    }
    const result = matches[0]!;
    const citation = await backend.resolveCitation(result.passageId);
    if (!citation.tidbit || !citation.excerpt.includes(CANARY_URL)) {
      throw new Error("startup canary citation lost its authored revision or URL");
    }
    return {
      citationState: citation.state,
      executionMode: response.executionMode,
      passageId: result.passageId,
      resolvedPassageId: citation.passageId,
      revisionId: citation.tidbit.revisionId,
      resultCount: response.results.length,
      canaryUrl: CANARY_URL,
    };
  }
  throw new Error("startup canary search did not become available");
}
