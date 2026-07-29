import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";

const PREPARE_QUIT_EVENT = "kosh://prepare-quit";
const QUIT_CANCELED_EVENT = "kosh://quit-canceled";

export interface PrepareQuitNotice {
  requestId: number;
}

export interface QuitCanceledNotice {
  requestId: number;
}

interface QuitParticipant {
  cancel: () => void;
  prepare: () => Promise<void>;
}

export interface QuitNative {
  acknowledge: (requestId: number, error: string | null) => Promise<void>;
  onCanceled: (listener: (notice: QuitCanceledNotice) => void) => Promise<() => void>;
  onPrepare: (listener: (notice: PrepareQuitNotice) => void) => Promise<() => void>;
}

const participants = new Set<QuitParticipant>();

export const quitNative: QuitNative = {
  acknowledge: (requestId, error) =>
    invoke<void>("acknowledge_quit", {
      error,
      requestId,
    }),
  onCanceled: (listener) =>
    listen<QuitCanceledNotice>(QUIT_CANCELED_EVENT, (event) => listener(event.payload)),
  onPrepare: (listener) =>
    listen<PrepareQuitNotice>(PREPARE_QUIT_EVENT, (event) => listener(event.payload)),
};

export function registerQuitParticipant(participant: QuitParticipant): () => void {
  participants.add(participant);
  return () => participants.delete(participant);
}

async function prepareParticipants(): Promise<void> {
  await Promise.all([...participants].map((participant) => participant.prepare()));
}

function cancelParticipants() {
  for (const participant of participants) participant.cancel();
}

export function QuitCoordinator({ native = quitNative }: { native?: QuitNative }) {
  const activeRequestId = useRef<number | null>(null);
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window) && native === quitNative) return;
    let active = true;
    let stopPrepare: (() => void) | undefined;
    let stopCanceled: (() => void) | undefined;
    const install = async () => {
      const [prepare, canceled] = await Promise.all([
        native.onPrepare((notice) => {
          activeRequestId.current = notice.requestId;
          void prepareParticipants()
            .then(() => native.acknowledge(notice.requestId, null))
            .catch(async (reason: unknown) => {
              const error = errorMessage(reason);
              try {
                await native.acknowledge(notice.requestId, error);
              } catch {
                activeRequestId.current = null;
                cancelParticipants();
              }
            });
        }),
        native.onCanceled((notice) => {
          if (activeRequestId.current !== notice.requestId) return;
          activeRequestId.current = null;
          cancelParticipants();
        }),
      ]);
      if (active) {
        stopPrepare = prepare;
        stopCanceled = canceled;
      } else {
        prepare();
        canceled();
      }
    };
    void install().catch((reason: unknown) => {
      console.error("Could not observe application quit requests", reason);
    });
    return () => {
      active = false;
      stopPrepare?.();
      stopCanceled?.();
    };
  }, [native]);

  return null;
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}
