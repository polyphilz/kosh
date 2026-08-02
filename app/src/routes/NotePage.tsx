import { useNavigate } from "@tanstack/react-router";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { useBackend } from "../backend/context";
import type {
  Backend,
  SelectedAttachmentRecord,
  TidbitRecord,
  WorkingCopyRecord,
} from "../backend/contracts";
import { KoshBlockNoteEditor, type KoshBlockNoteEditorHandle } from "../editor/KoshBlockNoteEditor";
import {
  NoteAutosaveCoordinator,
  hasMeaningfulAuthoredContent,
  type NoteMediaReservation,
} from "../notes/autosave";
import { projectLegacyTitle } from "../notes/legacyTitle";
import { registerQuitParticipant } from "../lifecycle/quit";
import { TauriEvent } from "../tauriProtocol";

interface FileDropNotice {
  selections: Array<{
    selectionId: string;
    filename: string;
  }>;
}

interface NotePageProps {
  mode: "durable" | "ephemeral";
  noteId: string;
}

interface NoteSession {
  coordinator: NoteAutosaveCoordinator;
  note: TidbitRecord | null;
}

const scrollPositions = new Map<string, number>();
const reconciliationStarted = new WeakSet<Backend>();

export function NotePage({ mode, noteId }: NotePageProps) {
  const backend = useBackend();
  const [session, setSession] = useState<NoteSession | null>(() =>
    mode === "ephemeral"
      ? { coordinator: NoteAutosaveCoordinator.ephemeral(backend, { noteId }), note: null }
      : null,
  );
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    if (mode === "ephemeral") {
      const initialCoordinator = session?.coordinator;
      void backend
        .loadWorkingCopy(noteId)
        .then((workingCopy) => {
          if (!active || !workingCopy || !initialCoordinator) return;
          const snapshot = initialCoordinator.getSnapshot();
          if (snapshot.editGeneration !== 0 || snapshot.bodyMarkdown !== "") return;
          setSession({
            coordinator: NoteAutosaveCoordinator.recovered(backend, workingCopy),
            note: null,
          });
        })
        .catch((reason: unknown) => {
          if (active) setLoadError(errorMessage(reason));
        });
      return () => {
        active = false;
      };
    }

    void Promise.all([backend.loadTidbit(noteId), backend.loadWorkingCopy(noteId)])
      .then(([note, workingCopy]) => {
        if (!active) return;
        setSession({
          coordinator: coordinatorForDurableNote(backend, note, workingCopy),
          note,
        });
      })
      .catch((reason: unknown) => {
        if (active) setLoadError(errorMessage(reason));
      });
    return () => {
      active = false;
    };
  }, [backend, mode, noteId]);

  if (loadError) {
    return (
      <main className="note-page note-page--error">
        <div role="alert">
          <p>Could not open this note.</p>
          <span>{loadError}</span>
        </div>
      </main>
    );
  }
  if (!session) {
    return (
      <main aria-busy="true" className="note-page">
        <span className="visually-hidden">Opening note</span>
      </main>
    );
  }
  return (
    <NoteEditorSession
      coordinator={session.coordinator}
      key={session.coordinator.getSnapshot().editGeneration === 0 ? "clean" : "recovered"}
      mode={mode}
      noteId={noteId}
    />
  );
}

interface NoteEditorSessionProps {
  coordinator: NoteAutosaveCoordinator;
  mode: NotePageProps["mode"];
  noteId: string;
}

function NoteEditorSession({ coordinator, mode, noteId }: NoteEditorSessionProps) {
  const backend = useBackend();
  const navigate = useNavigate();
  const editorRef = useRef<KoshBlockNoteEditorHandle>(null);
  const editorMediaPendingRef = useRef(false);
  const dropCountRef = useRef(0);
  const pendingWaitersRef = useRef(new Set<() => void>());
  const disposeTimerRef = useRef<number | null>(null);
  const [mediaPending, setMediaPending] = useState(false);
  const [mediaError, setMediaError] = useState<string | null>(null);
  const snapshot = useSyncExternalStore(coordinator.subscribe, coordinator.getSnapshot);

  const updatePendingState = useCallback(() => {
    const pending = editorMediaPendingRef.current || dropCountRef.current > 0;
    setMediaPending(pending);
    if (!pending) {
      for (const resolve of pendingWaitersRef.current) resolve();
      pendingWaitersRef.current.clear();
    }
  }, []);

  const waitForPendingMedia = useCallback(() => {
    if (!editorMediaPendingRef.current && dropCountRef.current === 0) return Promise.resolve();
    return new Promise<void>((resolve) => pendingWaitersRef.current.add(resolve));
  }, []);

  useEffect(() => {
    const frame = window.requestAnimationFrame(() => {
      editorRef.current?.focus();
      restoreScroll(noteId);
      scheduleWorkingCopyReconciliation(backend, noteId);
    });
    return () => window.cancelAnimationFrame(frame);
  }, [backend, noteId]);

  useEffect(() => {
    if (disposeTimerRef.current !== null) {
      window.clearTimeout(disposeTimerRef.current);
      disposeTimerRef.current = null;
    }
    return () => {
      scrollPositions.set(noteId, window.scrollY);
      for (const resolve of pendingWaitersRef.current) resolve();
      pendingWaitersRef.current.clear();
      disposeTimerRef.current = window.setTimeout(() => coordinator.dispose(), 0);
    };
  }, [coordinator, noteId]);

  useEffect(() => {
    if (mode !== "ephemeral" || snapshot.baseRevisionId === null) return;
    void navigate({
      to: "/notes/$noteId",
      params: { noteId },
      replace: true,
    });
  }, [mode, navigate, noteId, snapshot.baseRevisionId]);

  useEffect(
    () =>
      registerQuitParticipant({
        cancel: () => undefined,
        prepare: async (reason) => {
          await waitForPendingMedia();
          await coordinator.flush(reason);
        },
      }),
    [coordinator, waitForPendingMedia],
  );

  useEffect(() => {
    const onVisibilityChange = () => {
      if (document.visibilityState !== "hidden") return;
      void waitForPendingMedia()
        .then(() => coordinator.flush("HIDE"))
        .catch(() => undefined);
    };
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => document.removeEventListener("visibilitychange", onVisibilityChange);
  }, [coordinator, waitForPendingMedia]);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen<FileDropNotice>(
      TauriEvent.FileDrop,
      (event) => {
        const selectionIds = event.payload.selections.map((selection) => selection.selectionId);
        if (!active) {
          void backend.discardFileDropSelections(selectionIds);
          return;
        }
        dropCountRef.current += 1;
        updatePendingState();
        setMediaError(null);
        void ingestDroppedAttachments(backend, coordinator, event.payload.selections)
          .then(({ attachments, failures }) => {
            if (!active) return;
            if (attachments.length > 0) editorRef.current?.insertAttachments(attachments);
            if (failures.length > 0) {
              setMediaError(`Could not add dropped files: ${failures.join("; ")}`);
            }
          })
          .catch((reason: unknown) => {
            if (active) setMediaError(`Could not add dropped files: ${errorMessage(reason)}`);
            void backend.discardFileDropSelections(selectionIds);
          })
          .finally(() => {
            dropCountRef.current = Math.max(0, dropCountRef.current - 1);
            if (active) updatePendingState();
          });
      },
      { target: getCurrentWindow().label },
    ).then((stop) => {
      if (active) {
        unlisten = stop;
        void backend.setFileDropConsumerActive(true);
      } else {
        stop();
      }
    });
    return () => {
      active = false;
      unlisten?.();
      void backend.setFileDropConsumerActive(false);
    };
  }, [backend, coordinator, updatePendingState]);

  const withMediaReservation = useCallback(
    async <Record,>(operation: (draftId: string) => Promise<Record>): Promise<Record> => {
      const reservation = await coordinator.prepareMedia();
      try {
        return await operation(reservation.draftId);
      } catch (reason) {
        await discardFailedReservation(coordinator, reservation);
        throw reason;
      }
    },
    [coordinator],
  );

  const error = snapshot.error ?? mediaError;
  return (
    <main aria-busy={mediaPending || undefined} className="note-page">
      <div className="note-page__document">
        <KoshBlockNoteEditor
          ariaLabel="Note"
          attachmentStatus={(attachmentId) => backend.attachmentStatus(attachmentId)}
          imageStatus={(attachmentId) => backend.imageStatus(attachmentId)}
          onChange={(bodyMarkdown) => coordinator.update(bodyMarkdown)}
          onImageError={(reason) =>
            setMediaError(`Could not add attachment: ${errorMessage(reason)}`)
          }
          onPendingImagesChange={(pending) => {
            editorMediaPendingRef.current = pending;
            updatePendingState();
          }}
          openAttachmentExternal={(attachmentId) => backend.openAttachmentExternal(attachmentId)}
          openPdfExternal={(attachmentId) => backend.openPdfExternal(attachmentId)}
          pasteImage={async () => {
            const captureId = await backend.captureClipboardImage();
            return withMediaReservation((draftId) =>
              backend.ingestClipboardImage(captureId, draftId),
            );
          }}
          pdfStatus={(attachmentId) => backend.pdfStatus(attachmentId)}
          pickAttachment={async () => {
            const selectionId = await backend.selectAttachment();
            if (!selectionId) return null;
            return withMediaReservation((draftId) =>
              backend.ingestSelectedAttachment(selectionId, draftId),
            );
          }}
          pickImage={async () => {
            const selectionId = await backend.selectImage();
            if (!selectionId) return null;
            return withMediaReservation((draftId) =>
              backend.ingestSelectedImage(selectionId, draftId),
            );
          }}
          pickPdf={async () => {
            const selectionId = await backend.selectPdf();
            if (!selectionId) return null;
            return withMediaReservation((draftId) =>
              backend.ingestSelectedPdf(selectionId, draftId),
            );
          }}
          placeholder="Write something…"
          ref={editorRef}
          revealAttachmentInFinder={(attachmentId) =>
            backend.revealAttachmentInFinder(attachmentId)
          }
          retryImageOcr={(attachmentId) => backend.retryImageOcr(attachmentId)}
          retryPdfExtraction={(attachmentId) => backend.retryPdfExtraction(attachmentId)}
          value={snapshot.bodyMarkdown}
          variant="page"
        />
        {error && (
          <div className="note-page__error" role="alert">
            <span>{error}</span>
            {snapshot.error && (
              <button onClick={() => void coordinator.retry()} type="button">
                Retry
              </button>
            )}
          </div>
        )}
      </div>
    </main>
  );
}

function coordinatorForDurableNote(
  backend: Backend,
  note: TidbitRecord,
  workingCopy: WorkingCopyRecord | null,
): NoteAutosaveCoordinator {
  if (workingCopy) return NoteAutosaveCoordinator.recovered(backend, workingCopy);
  return new NoteAutosaveCoordinator(backend, {
    noteId: note.id,
    baseRevisionId: note.currentRevisionId,
    bodyMarkdown: projectLegacyTitle(note.title, note.bodyMarkdown),
    sources: note.sources.map(({ label, url }) => ({ label, url })),
  });
}

async function discardFailedReservation(
  coordinator: NoteAutosaveCoordinator,
  reservation: NoteMediaReservation,
): Promise<void> {
  try {
    await coordinator.discardMediaReservation(reservation);
  } catch (reason) {
    console.error("Could not discard a failed media reservation", reason);
  }
}

async function ingestDroppedAttachments(
  backend: Backend,
  coordinator: NoteAutosaveCoordinator,
  selections: FileDropNotice["selections"],
): Promise<{ attachments: SelectedAttachmentRecord[]; failures: string[] }> {
  const reservation = await coordinator.prepareMedia();
  const attachments: SelectedAttachmentRecord[] = [];
  const failures: string[] = [];
  for (const selection of selections) {
    try {
      attachments.push(
        await backend.ingestSelectedAttachment(selection.selectionId, reservation.draftId),
      );
    } catch (reason) {
      failures.push(`${selection.filename}: ${errorMessage(reason)}`);
    }
  }
  if (attachments.length === 0) await discardFailedReservation(coordinator, reservation);
  return { attachments, failures };
}

function scheduleWorkingCopyReconciliation(backend: Backend, activeNoteId: string): void {
  if (reconciliationStarted.has(backend)) return;
  reconciliationStarted.add(backend);
  window.setTimeout(() => {
    void backend
      .listWorkingCopies()
      .then(async (workingCopies) => {
        for (const workingCopy of workingCopies) {
          if (workingCopy.noteId === activeNoteId) continue;
          if (
            workingCopy.baseRevisionId === null &&
            !hasMeaningfulAuthoredContent(workingCopy.bodyMarkdown)
          ) {
            await backend.discardWorkingCopy({
              noteId: workingCopy.noteId,
              expectedEditGeneration: workingCopy.editGeneration,
            });
          } else {
            await backend.checkpointWorkingCopy({
              noteId: workingCopy.noteId,
              expectedEditGeneration: workingCopy.editGeneration,
            });
          }
        }
      })
      .catch((reason: unknown) => {
        console.error("Could not reconcile interrupted note autosaves", reason);
      });
  }, 0);
}

function restoreScroll(noteId: string): void {
  const scrollTop = scrollPositions.get(noteId);
  if (scrollTop === undefined) return;
  window.scrollTo({ behavior: "instant", top: scrollTop });
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}
