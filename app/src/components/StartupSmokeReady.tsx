import { emit } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { useBackend } from "../backend/context";
import type { Backend } from "../backend/contracts";
import { TauriEvent } from "../tauriProtocol";

interface StartupSmokeReadyProps {
  surface: "main" | "quick-add";
}

interface StartupCanaryEvidence {
  citationState: "CURRENT" | "HISTORICAL";
  executionMode: "EXACT" | "HYBRID" | "LEXICAL_ONLY";
  passageId: string;
  resolvedPassageId: string;
  revisionId: string;
  resultCount: number;
  sourceUrl: string;
}

export function StartupSmokeReady({ surface }: StartupSmokeReadyProps) {
  const backend = useBackend();

  useEffect(() => {
    if (!import.meta.env.DEV || !("__TAURI_INTERNALS__" in window)) return;

    let canceled = false;
    void (async () => {
      const probe = await backend.runtimeProbe();
      if (canceled) return;
      const canary = probe.startupSmokeCanary
        ? await proveSearchAndCitation(backend, probe.startupSmokeCanary)
        : null;
      if (canceled) return;
      const root = document.getElementById("root");
      await emit(TauriEvent.StartupSmokeReady, {
        surface,
        rendered: Boolean(root?.firstElementChild),
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

async function proveSearchAndCitation(
  backend: Backend,
  query: string,
): Promise<StartupCanaryEvidence> {
  const response = await backend.searchPassages({
    query,
    mode: "EXACT",
    limit: 10,
  });
  const matches = response.results.filter(({ citation }) => citation.excerpt.includes(query));
  if (matches.length !== 1) {
    throw new Error(`startup canary search returned ${matches.length} matching passages`);
  }
  const result = matches[0]!;
  const citation = await backend.resolveCitation(result.passageId);
  const sourceUrl = citation.sources.find((source) => source.url !== null)?.url;
  if (!citation.tidbit || !sourceUrl) {
    throw new Error("startup canary citation lost its authored revision or source URL");
  }
  return {
    citationState: citation.state,
    executionMode: response.executionMode,
    passageId: result.passageId,
    resolvedPassageId: citation.passageId,
    revisionId: citation.tidbit.revisionId,
    resultCount: response.results.length,
    sourceUrl,
  };
}
