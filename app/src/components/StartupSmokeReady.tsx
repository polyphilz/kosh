import { emit } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { useBackend } from "../backend/context";
import type { Backend } from "../backend/contracts";
import { createUuidV7, NoteAutosaveCoordinator } from "../notes/autosave";
import { TauriEvent } from "../tauriProtocol";

interface StartupSmokeReadyProps {
  surface: "main" | "quick-add";
}

const CANARY_SOURCE_LABEL = "Kosh startup smoke";
const CANARY_SOURCE_URL = "https://example.invalid/kosh-progressive-operability";
let startupCapture: Promise<boolean> | undefined;

interface StartupCanaryEvidence {
  blockId: string;
  executionMode: "EXACT" | "HYBRID" | "LEXICAL_ONLY";
  noteId: string;
  revisionId: string;
  resultCount: number;
  sourceUrl: string;
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
      const canary = await proveCurrentBlockSearch(backend, probe.startupSmokeCanary);
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
  const existing = await backend.searchBlocks({
    query,
    mode: "EXACT",
    limit: 10,
  });
  if (existing.results.some(({ excerpt }) => excerpt.includes(query))) return false;
  const coordinator = NoteAutosaveCoordinator.ephemeral(backend, { noteId: createUuidV7() });
  coordinator.update(query, [{ label: CANARY_SOURCE_LABEL, url: CANARY_SOURCE_URL }]);
  await coordinator.flush("IDLE");
  coordinator.dispose();
  return true;
}

async function proveCurrentBlockSearch(
  backend: Backend,
  query: string,
): Promise<StartupCanaryEvidence> {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    const response = await backend.searchBlocks({
      query,
      mode: "EXACT",
      limit: 10,
    });
    const matches = response.results.filter(({ excerpt }) => excerpt.includes(query));
    if (matches.length > 1) {
      throw new Error(`startup canary search returned ${matches.length} matching blocks`);
    }
    if (matches.length === 0) {
      await new Promise((resolve) => setTimeout(resolve, 100));
      continue;
    }
    const result = matches[0]!;
    const note = await backend.loadTidbit(result.noteId);
    const sourceUrl = note.sources.find((source) => source.url !== null)?.url;
    if (!sourceUrl || note.currentRevisionId.length === 0) {
      throw new Error("startup canary block lost its current note or source URL");
    }
    return {
      blockId: result.blockId,
      executionMode: response.executionMode,
      noteId: result.noteId,
      revisionId: note.currentRevisionId,
      resultCount: response.results.length,
      sourceUrl,
    };
  }
  throw new Error("startup canary search did not become available");
}
