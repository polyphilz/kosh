import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
  useSyncExternalStore,
  type KeyboardEvent,
} from "react";
import { useBackend } from "../backend/context";
import type { Backend, SelectedAttachmentRecord } from "../backend/contracts";
import { KoshBlockNoteEditor, type KoshBlockNoteEditorHandle } from "../editor/KoshBlockNoteEditor";
import { registerQuitParticipant } from "../lifecycle/quit";
import {
  createUuidV7,
  NoteAutosaveCoordinator,
  type NoteMediaReservation,
} from "../notes/autosave";
import { NoteActions } from "../notes/NoteActions";
import { TauriEvent } from "../tauriProtocol";
import {
  QuickAddDismissAction,
  quickAddNative,
  type QuickAddDismissRequest,
  type QuickAddNative,
} from "./native";

interface FileDropNotice {
  selections: Array<{
    selectionId: string;
    filename: string;
  }>;
}

interface QuickAddWindowProps {
  native?: QuickAddNative;
}

interface QuickAddSessionHandle {
  focus: () => void;
  isEditorOverlayOpen: () => boolean;
  requestDismiss: (action: QuickAddDismissAction) => Promise<boolean>;
}

export function QuickAddWindow({ native = quickAddNative }: QuickAddWindowProps) {
  const sessionRef = useRef<QuickAddSessionHandle>(null);
  const [generation, setGeneration] = useState(0);
  const focusEditor = useCallback(() => {
    window.requestAnimationFrame(() => sessionRef.current?.focus());
  }, []);
  const requestDismiss = useCallback((action: QuickAddDismissAction) => {
    void sessionRef.current?.requestDismiss(action);
  }, []);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window) && native === quickAddNative) {
      focusEditor();
      return;
    }
    let active = true;
    let stopShown: (() => void) | undefined;
    let stopDismiss: (() => void) | undefined;
    void Promise.all([
      native.onShown(focusEditor),
      native.onDismissRequested((request: QuickAddDismissRequest) =>
        requestDismiss(request.action),
      ),
    ])
      .then(([shown, dismiss]) => {
        if (active) {
          stopShown = shown;
          stopDismiss = dismiss;
        } else {
          shown();
          dismiss();
        }
      })
      .catch((reason: unknown) =>
        console.error("Could not observe quick-add window events", reason),
      );
    return () => {
      active = false;
      stopShown?.();
      stopDismiss?.();
    };
  }, [focusEditor, native, requestDismiss]);

  const handleKeyboard = (event: KeyboardEvent<HTMLElement>) => {
    if (event.nativeEvent.isComposing) return;
    const target = event.target;
    if (target instanceof Element && target.closest('[role="dialog"]')) return;
    if (
      event.key === "Enter" &&
      (event.metaKey || event.ctrlKey) &&
      !event.altKey &&
      !event.shiftKey
    ) {
      event.preventDefault();
      event.stopPropagation();
      requestDismiss(QuickAddDismissAction.Dismiss);
      return;
    }
    if (event.key !== "Escape" || sessionRef.current?.isEditorOverlayOpen()) return;
    event.preventDefault();
    event.stopPropagation();
    requestDismiss(QuickAddDismissAction.Dismiss);
  };

  return (
    <main aria-label="Quick add" className="quick-add-shell" onKeyDownCapture={handleKeyboard}>
      <section className="quick-add-card">
        <header className="quick-add-card__header">
          <div>
            <p className="page-kicker">Jot and forget</p>
            <h1>Quick add</h1>
          </div>
          <span>⌘↵ or Esc to finish</span>
        </header>
        <QuickAddSession
          key={generation}
          native={native}
          onDismissed={() => setGeneration((value) => value + 1)}
          ref={sessionRef}
        />
      </section>
    </main>
  );
}

const QuickAddSession = forwardRef<
  QuickAddSessionHandle,
  { native: QuickAddNative; onDismissed: () => void }
>(function QuickAddSession({ native, onDismissed }, ref) {
  const backend = useBackend();
  const editorRef = useRef<KoshBlockNoteEditorHandle>(null);
  const editorMediaPendingRef = useRef(false);
  const dropCountRef = useRef(0);
  const pendingWaitersRef = useRef(new Set<() => void>());
  const finishPromiseRef = useRef<Promise<boolean> | null>(null);
  const mediaFailureRef = useRef<{ message: string; version: number } | null>(null);
  const mediaFailureVersionRef = useRef(0);
  const handledMediaFailureVersionRef = useRef(0);
  const [coordinator] = useState(() =>
    NoteAutosaveCoordinator.ephemeral(backend, { noteId: createUuidV7() }),
  );
  const snapshot = useSyncExternalStore(coordinator.subscribe, coordinator.getSnapshot);
  const [mediaPending, setMediaPending] = useState(false);
  const [mediaError, setMediaError] = useState<string | null>(null);
  const [finishError, setFinishError] = useState<string | null>(null);
  const [finishing, setFinishing] = useState(false);
  const [retryAction, setRetryAction] = useState<QuickAddDismissAction>(
    QuickAddDismissAction.Dismiss,
  );

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
  const reportMediaError = useCallback((message: string) => {
    const version = mediaFailureVersionRef.current + 1;
    mediaFailureVersionRef.current = version;
    mediaFailureRef.current = { message, version };
    setMediaError(message);
  }, []);
  const clearMediaError = useCallback(() => {
    handledMediaFailureVersionRef.current = mediaFailureVersionRef.current;
    mediaFailureRef.current = null;
    setMediaError(null);
  }, []);
  const waitForSettledMedia = useCallback(async () => {
    await waitForPendingMedia();
    const failure = mediaFailureRef.current;
    if (!failure || failure.version <= handledMediaFailureVersionRef.current) return;
    handledMediaFailureVersionRef.current = failure.version;
    throw new Error(failure.message);
  }, [waitForPendingMedia]);
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
  const withFileDialog = useCallback(
    async <Result,>(operation: () => Promise<Result>): Promise<Result> => {
      await native.setFileDialogOpen(true);
      try {
        return await operation();
      } finally {
        await native.setFileDialogOpen(false);
      }
    },
    [native],
  );

  const requestDismiss = useCallback(
    (action: QuickAddDismissAction): Promise<boolean> => {
      if (finishPromiseRef.current) return finishPromiseRef.current;
      setRetryAction(action);
      setFinishError(null);
      setFinishing(true);
      const operation = (async () => {
        try {
          await waitForSettledMedia();
          await coordinator.flush("HIDE");
          await native.dismiss(action);
          onDismissed();
          return true;
        } catch (reason) {
          const message = `Could not save Quick Add: ${errorMessage(reason)}`;
          try {
            await native.cancelDismiss();
            setFinishError(message);
          } catch (cancelReason) {
            setFinishError(`${message} Could not reset dismissal: ${errorMessage(cancelReason)}`);
          }
          return false;
        } finally {
          finishPromiseRef.current = null;
          setFinishing(false);
        }
      })();
      finishPromiseRef.current = operation;
      return operation;
    },
    [coordinator, native, onDismissed, waitForSettledMedia],
  );

  useImperativeHandle(
    ref,
    () => ({
      focus: () => editorRef.current?.focus(),
      isEditorOverlayOpen: () => editorRef.current?.isSuggestionMenuOpen() ?? false,
      requestDismiss,
    }),
    [requestDismiss],
  );

  useEffect(() => () => coordinator.dispose(), [coordinator]);
  useEffect(
    () =>
      registerQuitParticipant({
        cancel: () => setFinishing(false),
        prepare: async (reason) => {
          setFinishing(true);
          await waitForSettledMedia();
          await coordinator.flush(reason);
        },
      }),
    [coordinator, waitForSettledMedia],
  );

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
        if (event.payload.selections.length === 0) return;
        clearMediaError();
        dropCountRef.current += 1;
        updatePendingState();
        void ingestDroppedAttachments(backend, coordinator, event.payload.selections)
          .then(({ attachments, failures }) => {
            if (!active) return;
            if (attachments.length > 0) editorRef.current?.insertAttachments(attachments);
            if (failures.length > 0) {
              reportMediaError(`Could not add dropped files: ${failures.join("; ")}`);
            }
          })
          .catch((reason: unknown) => {
            if (active) reportMediaError(`Could not add dropped files: ${errorMessage(reason)}`);
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
  }, [backend, clearMediaError, coordinator, reportMediaError, updatePendingState]);

  const error = finishError ?? snapshot.error ?? mediaError;
  return (
    <div className="quick-add-editor" data-phase={snapshot.phase}>
      <NoteActions
        canDelete={false}
        deleteError={null}
        deleting={false}
        disabled={finishing || mediaPending}
        onDelete={() => undefined}
        onSourcesChange={(sources) =>
          coordinator.update(coordinator.getSnapshot().bodyMarkdown, sources)
        }
        sources={snapshot.sources}
      />
      <KoshBlockNoteEditor
        ariaLabel="Quick note"
        attachmentStatus={(attachmentId) => backend.attachmentStatus(attachmentId)}
        disabled={finishing}
        imageStatus={(attachmentId) => backend.imageStatus(attachmentId)}
        onChange={(bodyMarkdown) => coordinator.update(bodyMarkdown)}
        onImageError={(reason) =>
          reportMediaError(`Could not add attachment: ${errorMessage(reason)}`)
        }
        onPendingImagesChange={(pending) => {
          editorMediaPendingRef.current = pending;
          updatePendingState();
        }}
        openAttachmentExternal={(attachmentId) => backend.openAttachmentExternal(attachmentId)}
        openPdfExternal={(attachmentId) => backend.openPdfExternal(attachmentId)}
        pasteImage={async () => {
          clearMediaError();
          const captureId = await backend.captureClipboardImage();
          return withMediaReservation((draftId) =>
            backend.ingestClipboardImage(captureId, draftId),
          );
        }}
        pdfStatus={(attachmentId) => backend.pdfStatus(attachmentId)}
        pickAttachment={async () => {
          clearMediaError();
          const selectionId = await withFileDialog(() => backend.selectAttachment());
          if (!selectionId) return null;
          return withMediaReservation((draftId) =>
            backend.ingestSelectedAttachment(selectionId, draftId),
          );
        }}
        pickImage={async () => {
          clearMediaError();
          const selectionId = await withFileDialog(() => backend.selectImage());
          if (!selectionId) return null;
          return withMediaReservation((draftId) =>
            backend.ingestSelectedImage(selectionId, draftId),
          );
        }}
        pickPdf={async () => {
          clearMediaError();
          const selectionId = await withFileDialog(() => backend.selectPdf());
          if (!selectionId) return null;
          return withMediaReservation((draftId) => backend.ingestSelectedPdf(selectionId, draftId));
        }}
        placeholder="Write something…"
        ref={editorRef}
        revealAttachmentInFinder={(attachmentId) => backend.revealAttachmentInFinder(attachmentId)}
        retryImageOcr={(attachmentId) => backend.retryImageOcr(attachmentId)}
        retryPdfExtraction={(attachmentId) => backend.retryPdfExtraction(attachmentId)}
        value={snapshot.bodyMarkdown}
        variant="page"
      />
      {error && (
        <div className="quick-add-editor__error" role="alert">
          <span>{error}</span>
          <button
            disabled={finishing || mediaPending}
            onClick={() => void requestDismiss(retryAction)}
            type="button"
          >
            Retry
          </button>
        </div>
      )}
    </div>
  );
});

async function discardFailedReservation(
  coordinator: NoteAutosaveCoordinator,
  reservation: NoteMediaReservation,
): Promise<void> {
  try {
    await coordinator.discardMediaReservation(reservation);
  } catch (reason) {
    console.error("Could not discard a failed Quick Add media reservation", reason);
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

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}
