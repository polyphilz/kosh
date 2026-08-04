import { useBlocker, useNavigate } from "@tanstack/react-router";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { useBackend } from "../backend/context";
import type {
  Backend,
  CitationResolution,
  SelectedAttachmentRecord,
  TidbitRecord,
  WorkingCopyRecord,
} from "../backend/contracts";
import {
  KOSH_NOTE_PLACEHOLDER,
  KoshBlockNoteEditor,
  type KoshBlockNoteEditorHandle,
} from "../editor/KoshBlockNoteEditor";
import {
  clearFindInNoteTransfer,
  consumeFindInNoteTransfer,
  consumeFindInNoteRequest,
  FIND_IN_NOTE_REQUEST_EVENT,
  transferFindInNote,
  type FindInNoteResult,
} from "../editor/findInNote";
import {
  createUuidV7,
  NoteAutosaveCoordinator,
  type NoteMediaReservation,
} from "../notes/autosave";
import { NoteActions } from "../notes/NoteActions";
import { useShortcutSettings } from "../shortcuts/context";
import {
  DEFAULT_DELETE_NOTE_ACCELERATOR,
  LocalShortcutCommand,
  localBindingFor,
} from "../shortcuts/localShortcuts";
import { hasMeaningfulAuthoredContent } from "../notes/content";
import { useNoteDeletion } from "../notes/deletion";
import { registerQuitParticipant } from "../lifecycle/quit";
import { registerSearchCheckpoint } from "../search/checkpoint";
import { citationLocation, citationOwner } from "../search/presentation";
import {
  SEARCH_RESULT_SELECTED_EVENT,
  type SearchResultSelectedDetail,
} from "../search/SearchOverlay";
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
  passageId?: string;
}

interface NoteSession {
  coordinator: NoteAutosaveCoordinator;
  noteId: string;
  note: TidbitRecord | null;
}

const scrollPositions = new Map<string, number>();
const reconciliationStarted = new WeakSet<Backend>();
const activeNoteIds = new WeakMap<Backend, string>();
const reconciliationOperations = new WeakMap<Backend, Map<string, Promise<void>>>();
const pendingDeleteDialogTransfers = new Set<string>();
const SEARCH_MATCH_FLASH_MS = 1_400;
const EMPTY_FIND_RESULT: FindInNoteResult = { activeIndex: -1, count: 0 };

export function NotePage({ mode, noteId, passageId }: NotePageProps) {
  const backend = useBackend();
  const navigate = useNavigate();
  const [session, setSession] = useState<NoteSession | null>(null);
  const [loadError, setLoadError] = useState<{ message: string; noteId: string } | null>(null);

  useEffect(() => {
    activeNoteIds.set(backend, noteId);
    return () => {
      if (activeNoteIds.get(backend) === noteId) activeNoteIds.delete(backend);
    };
  }, [backend, noteId]);

  useEffect(() => {
    let active = true;
    setLoadError(null);
    void loadNoteSession(backend, mode, noteId)
      .then((nextSession) => {
        if (!active) return;
        setSession(nextSession);
      })
      .catch((reason: unknown) => {
        if (!active) return;
        if (reason instanceof DeletedNoteError) {
          void navigate({
            to: "/new/$noteId",
            params: { noteId: createUuidV7() },
            replace: true,
          });
          return;
        }
        setLoadError({ message: errorMessage(reason), noteId });
      });
    return () => {
      active = false;
    };
  }, [backend, mode, navigate, noteId]);

  if (loadError?.noteId === noteId) {
    return (
      <main className="note-page note-page--error">
        <div role="alert">
          <p>Could not open this note.</p>
          <span>{loadError.message}</span>
        </div>
      </main>
    );
  }
  if (!session || session.noteId !== noteId) {
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
      passageId={passageId}
    />
  );
}

interface NoteEditorSessionProps {
  coordinator: NoteAutosaveCoordinator;
  mode: NotePageProps["mode"];
  noteId: string;
  passageId?: string;
}

function NoteEditorSession({ coordinator, mode, noteId, passageId }: NoteEditorSessionProps) {
  const backend = useBackend();
  const announceDeletedNote = useNoteDeletion();
  const navigate = useNavigate();
  const { localBindings } = useShortcutSettings();
  const editorRef = useRef<KoshBlockNoteEditorHandle>(null);
  const findInputRef = useRef<HTMLInputElement>(null);
  const editorMediaPendingRef = useRef(false);
  const dropCountRef = useRef(0);
  const pendingWaitersRef = useRef(new Set<() => void>());
  const disposeTimerRef = useRef<number | null>(null);
  const leavingNoteRef = useRef(false);
  const lifecyclePreparingRef = useRef(false);
  const lifecyclePreparationRef = useRef<Promise<void> | null>(null);
  const [lifecyclePreparing, setLifecyclePreparing] = useState(false);
  const [mediaPending, setMediaPending] = useState(false);
  const [mediaError, setMediaError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(() => pendingDeleteDialogTransfers.delete(noteId));
  const findRequestRoute = `/${mode === "ephemeral" ? "new" : "notes"}/${noteId}`;
  const [initialFindTransfer] = useState(() => consumeFindInNoteTransfer(findRequestRoute));
  const [findOpen, setFindOpen] = useState(initialFindTransfer !== null);
  const [findState, setFindState] = useState({
    ...EMPTY_FIND_RESULT,
    activeIndex: initialFindTransfer?.activeIndex ?? EMPTY_FIND_RESULT.activeIndex,
    query: initialFindTransfer?.query ?? "",
  });
  const [searchFocus, setSearchFocus] = useState<SearchFocusState | null>(null);
  const [searchSelectionRevision, setSearchSelectionRevision] = useState(0);
  const snapshot = useSyncExternalStore(coordinator.subscribe, coordinator.getRenderedSnapshot);
  const editorInitialValue = useRef(coordinator.getSnapshot().bodyMarkdown).current;
  const deleteShortcut =
    localBindingFor(localBindings, LocalShortcutCommand.DeleteNote)?.accelerator ??
    DEFAULT_DELETE_NOTE_ACCELERATOR;

  const updateFindState = useCallback((query: string, activeIndex = 0) => {
    const result = editorRef.current?.findInNote(query, activeIndex) ?? EMPTY_FIND_RESULT;
    setFindState({ ...result, query });
  }, []);

  const moveFind = useCallback((direction: "next" | "previous") => {
    const result = editorRef.current?.moveFindInNote(direction) ?? EMPTY_FIND_RESULT;
    setFindState((current) => ({ ...current, ...result }));
  }, []);

  const closeFind = useCallback(() => {
    editorRef.current?.clearFindInNote();
    clearFindInNoteTransfer(`/notes/${noteId}`);
    setFindOpen(false);
    window.requestAnimationFrame(() => editorRef.current?.focus());
  }, [noteId]);

  useLayoutEffect(() => {
    if (!findOpen) return;
    setFindState((current) => ({
      ...current,
      ...(editorRef.current?.findInNote(current.query, current.activeIndex) ?? EMPTY_FIND_RESULT),
    }));
    findInputRef.current?.focus();
    findInputRef.current?.select();
  }, [findOpen]);

  useEffect(() => {
    const openFind = () => {
      setFindOpen(true);
      setFindState((current) => {
        const result =
          editorRef.current?.findInNote(current.query, current.activeIndex) ?? EMPTY_FIND_RESULT;
        return { ...current, ...result };
      });
      window.requestAnimationFrame(() => {
        findInputRef.current?.focus();
        findInputRef.current?.select();
      });
    };
    const onFindRequest = (event: Event) => {
      if (
        !(event instanceof CustomEvent) ||
        event.detail !== findRequestRoute ||
        !consumeFindInNoteRequest(findRequestRoute)
      ) {
        return;
      }
      openFind();
    };
    window.addEventListener(FIND_IN_NOTE_REQUEST_EVENT, onFindRequest);
    if (consumeFindInNoteRequest(findRequestRoute)) openFind();
    return () => window.removeEventListener(FIND_IN_NOTE_REQUEST_EVENT, onFindRequest);
  }, [findRequestRoute]);

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

  const cancelLifecyclePreparation = useCallback(() => {
    lifecyclePreparationRef.current = null;
    lifecyclePreparingRef.current = false;
    setLifecyclePreparing(false);
  }, []);

  const prepareForLifecycle = useCallback(
    (reason: "QUIT" | "UPDATE_RESTART") => {
      const current = lifecyclePreparationRef.current;
      if (current) return current;
      lifecyclePreparingRef.current = true;
      setLifecyclePreparing(true);
      const operation = waitForPendingMedia().then(async () => {
        await coordinator.flush(reason);
      });
      lifecyclePreparationRef.current = operation;
      return operation.catch((error: unknown) => {
        if (lifecyclePreparationRef.current === operation) cancelLifecyclePreparation();
        throw error;
      });
    },
    [cancelLifecyclePreparation, coordinator, waitForPendingMedia],
  );

  const flushForNavigation = useCallback(async () => {
    await waitForPendingMedia();
    await coordinator.flush("NAVIGATION");
  }, [coordinator, waitForPendingMedia]);

  useEffect(() => registerSearchCheckpoint(flushForNavigation), [flushForNavigation]);

  useBlocker({
    enableBeforeUnload: false,
    shouldBlockFn: async ({ next }) => {
      leavingNoteRef.current =
        next.pathname !== `/new/${noteId}` && next.pathname !== `/notes/${noteId}`;
      try {
        await flushForNavigation();
        return false;
      } catch {
        leavingNoteRef.current = false;
        return true;
      }
    },
  });

  useEffect(() => {
    const repeatSelection = (event: Event) => {
      const detail = (event as CustomEvent<SearchResultSelectedDetail>).detail;
      if (detail.noteId === noteId && detail.passageId === passageId) {
        setSearchSelectionRevision((current) => current + 1);
      }
    };
    window.addEventListener(SEARCH_RESULT_SELECTED_EVENT, repeatSelection);
    return () => window.removeEventListener(SEARCH_RESULT_SELECTED_EVENT, repeatSelection);
  }, [noteId, passageId]);

  useEffect(() => {
    const frame = window.requestAnimationFrame(() => {
      const findInput = findInputRef.current;
      if (findInput) {
        findInput.focus();
        findInput.select();
      } else {
        editorRef.current?.focus();
      }
      restoreScroll(noteId);
      scheduleWorkingCopyReconciliation(backend);
    });
    return () => window.cancelAnimationFrame(frame);
  }, [backend, noteId]);

  useEffect(() => {
    editorRef.current?.clearSearchFocus();
    if (!passageId) {
      setSearchFocus(null);
      return;
    }
    let active = true;
    let flashTimer: number | null = null;
    setSearchFocus({ phase: "LOADING" });
    void backend
      .resolveCitation(passageId)
      .then((citation) => {
        if (!active) return;
        if (citation.tidbit && citation.tidbit.id !== noteId) {
          setSearchFocus({
            phase: "UNAVAILABLE",
            message: "This search result belongs to a different note.",
            citation,
          });
          return;
        }
        if (citation.state === "HISTORICAL") {
          setSearchFocus({
            phase: "HISTORICAL",
            message: "This exact passage is from an older revision; the current note is open.",
            citation,
          });
          return;
        }
        window.requestAnimationFrame(() => {
          if (!active) return;
          const focusCitation = () => editorRef.current?.focusCitation(citation) ?? false;
          const focused = focusCitation();
          setSearchFocus(
            focused
              ? { phase: "FOCUSED", citation }
              : {
                  phase: "UNAVAILABLE",
                  message: "The cited passage is no longer present in this note.",
                  citation,
                },
          );
          if (focused) {
            window.requestAnimationFrame(() => {
              if (active) focusCitation();
            });
            flashTimer = window.setTimeout(() => {
              if (!active) return;
              editorRef.current?.clearSearchFocus();
              setSearchFocus((current) =>
                current?.phase === "FOCUSED" && current.citation.passageId === citation.passageId
                  ? { phase: "EVIDENCE", citation: current.citation }
                  : current,
              );
            }, SEARCH_MATCH_FLASH_MS);
          }
        });
      })
      .catch((reason: unknown) => {
        if (active) {
          setSearchFocus({
            phase: "UNAVAILABLE",
            message: `Could not resolve this passage: ${errorMessage(reason)}`,
          });
        }
      });
    return () => {
      active = false;
      if (flashTimer !== null) window.clearTimeout(flashTimer);
      editorRef.current?.clearSearchFocus();
    };
  }, [backend, noteId, passageId, searchSelectionRevision]);

  useEffect(() => {
    if (disposeTimerRef.current !== null) {
      window.clearTimeout(disposeTimerRef.current);
      disposeTimerRef.current = null;
    }
    return () => {
      scrollPositions.set(noteId, window.scrollY);
      disposeTimerRef.current = window.setTimeout(() => {
        void flushForNavigation()
          .catch((reason: unknown) => {
            console.error("Could not flush note before navigation", reason);
          })
          .finally(() => coordinator.dispose());
      }, 0);
    };
  }, [coordinator, flushForNavigation, noteId]);

  useEffect(() => {
    if (mode !== "ephemeral" || snapshot.baseRevisionId === null || leavingNoteRef.current) {
      return;
    }
    const durableRoute = `/notes/${noteId}`;
    if (findOpen) transferFindInNote(durableRoute, findState.query, findState.activeIndex);
    if (deleteOpen) pendingDeleteDialogTransfers.add(noteId);
    void navigate({
      to: "/notes/$noteId",
      params: { noteId },
      replace: true,
    }).catch(() => {
      clearFindInNoteTransfer(durableRoute);
      pendingDeleteDialogTransfers.delete(noteId);
    });
  }, [
    deleteOpen,
    findOpen,
    findState.activeIndex,
    findState.query,
    mode,
    navigate,
    noteId,
    snapshot.baseRevisionId,
  ]);

  useEffect(
    () =>
      registerQuitParticipant({
        cancel: cancelLifecyclePreparation,
        prepare: prepareForLifecycle,
      }),
    [cancelLifecyclePreparation, prepareForLifecycle],
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
        if (!active || lifecyclePreparingRef.current) {
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
      if (lifecyclePreparingRef.current) {
        throw new Error("The note is preparing for an application lifecycle action");
      }
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

  const deleteCurrentNote = useCallback(async () => {
    if (deleting) return;
    setDeleting(true);
    setActionError(null);
    try {
      await flushForNavigation();
      const expectedRevisionId = coordinator.getSnapshot().baseRevisionId;
      if (!expectedRevisionId) throw new Error("This note has not been saved yet.");
      const deleted = await backend.deleteTidbit({ id: noteId, expectedRevisionId });
      announceDeletedNote(deleted);
      leavingNoteRef.current = true;
      await navigate({
        to: "/new/$noteId",
        params: { noteId: createUuidV7() },
        replace: true,
      });
    } catch (reason) {
      setActionError(`Could not delete note: ${errorMessage(reason)}`);
    } finally {
      setDeleting(false);
    }
  }, [announceDeletedNote, backend, coordinator, deleting, flushForNavigation, navigate, noteId]);

  const dismissSearchFocus = useCallback(() => {
    if (!passageId) {
      setSearchFocus(null);
      return;
    }
    void navigate({
      to: "/notes/$noteId",
      params: { noteId },
      search: {},
      replace: true,
    });
  }, [navigate, noteId, passageId]);

  const error = snapshot.error ?? mediaError ?? actionError;
  return (
    <main aria-busy={mediaPending || lifecyclePreparing || undefined} className="note-page">
      <h1 className="visually-hidden">Note</h1>
      {findOpen && (
        <NoteFindBar
          inputRef={findInputRef}
          onClose={closeFind}
          onMove={moveFind}
          onQueryChange={updateFindState}
          state={findState}
        />
      )}
      <NoteActions
        canEditSources={
          snapshot.baseRevisionId !== null || hasMeaningfulAuthoredContent(snapshot.bodyMarkdown)
        }
        canDelete={
          snapshot.baseRevisionId !== null || hasMeaningfulAuthoredContent(snapshot.bodyMarkdown)
        }
        deleteError={actionError}
        deleteOpen={deleteOpen}
        deleteShortcut={deleteShortcut}
        deleting={deleting}
        disabled={mediaPending || lifecyclePreparing}
        onDelete={() => void deleteCurrentNote()}
        onDeleteOpenChange={setDeleteOpen}
        onSourcesChange={(sources) => {
          setActionError(null);
          const current = coordinator.getSnapshot();
          coordinator.update(current.bodyMarkdown, sources);
        }}
        sources={snapshot.sources}
      />
      <div className="note-page__document">
        {searchFocus?.phase === "FOCUSED" && (
          <span aria-live="polite" className="visually-hidden" role="status">
            Search match
          </span>
        )}
        {searchFocus &&
          (searchFocus.phase === "HISTORICAL" || searchFocus.phase === "UNAVAILABLE") && (
            <SearchIntegrityNotice focus={searchFocus} onDismiss={dismissSearchFocus} />
          )}
        {searchFocus &&
          (searchFocus.phase === "FOCUSED" || searchFocus.phase === "EVIDENCE") &&
          searchFocus.citation.attachment && (
            <SearchEvidenceNotice citation={searchFocus.citation} onDismiss={dismissSearchFocus} />
          )}
        <KoshBlockNoteEditor
          ariaLabel="Note"
          attachmentStatus={(attachmentId) => backend.attachmentStatus(attachmentId)}
          disabled={lifecyclePreparing || deleting}
          imageStatus={(attachmentId) => backend.imageStatus(attachmentId)}
          onChange={(bodyMarkdown) => {
            coordinator.update(bodyMarkdown);
            if (
              searchFocus?.phase === "FOCUSED" &&
              !editorRef.current?.revalidateCitationFocus(searchFocus.citation)
            ) {
              setSearchFocus({
                phase: "UNAVAILABLE",
                message: "The cited passage is no longer present in this note.",
                citation: searchFocus.citation,
              });
            }
          }}
          onFindStateChange={() => {
            if (findOpen) updateFindState(findState.query, findState.activeIndex);
          }}
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
          placeholder={KOSH_NOTE_PLACEHOLDER}
          ref={editorRef}
          revealAttachmentInFinder={(attachmentId) =>
            backend.revealAttachmentInFinder(attachmentId)
          }
          retryImageOcr={(attachmentId) => backend.retryImageOcr(attachmentId)}
          retryPdfExtraction={(attachmentId) => backend.retryPdfExtraction(attachmentId)}
          selectionRail
          value={editorInitialValue}
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

interface NoteFindBarProps {
  inputRef: React.RefObject<HTMLInputElement | null>;
  onClose: () => void;
  onMove: (direction: "next" | "previous") => void;
  onQueryChange: (query: string) => void;
  state: FindInNoteResult & { query: string };
}

function NoteFindBar({ inputRef, onClose, onMove, onQueryChange, state }: NoteFindBarProps) {
  const status = !state.query
    ? "Type to find"
    : state.count === 0
      ? "No matches"
      : `${state.activeIndex + 1} of ${state.count}`;
  return (
    <section
      aria-label="Find in note"
      className="note-find"
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing || event.key !== "Escape") return;
        event.preventDefault();
        event.stopPropagation();
        onClose();
      }}
      role="search"
    >
      <input
        aria-label="Find in note"
        autoComplete="off"
        data-kosh-note-find-input
        maxLength={256}
        onChange={(event) => onQueryChange(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing) return;
          if (event.key === "Enter") {
            event.preventDefault();
            onMove(event.shiftKey ? "previous" : "next");
          }
        }}
        placeholder="Find in note"
        ref={inputRef}
        spellCheck={false}
        type="search"
        value={state.query}
      />
      <span aria-live="polite" className="note-find__status" role="status">
        {status}
      </span>
      <button
        aria-label="Previous match"
        disabled={state.count === 0}
        onClick={() => onMove("previous")}
        title="Previous match (Shift-Enter)"
        type="button"
      >
        ↑
      </button>
      <button
        aria-label="Next match"
        disabled={state.count === 0}
        onClick={() => onMove("next")}
        title="Next match (Enter)"
        type="button"
      >
        ↓
      </button>
      <button aria-label="Close find" onClick={onClose} title="Close (Escape)" type="button">
        ×
      </button>
    </section>
  );
}

type SearchFocusState =
  | { phase: "LOADING" }
  | {
      citation?: CitationResolution;
      message: string;
      phase: "HISTORICAL" | "UNAVAILABLE";
    }
  | { citation: CitationResolution; phase: "EVIDENCE" | "FOCUSED" };

function SearchEvidenceNotice({
  citation,
  onDismiss,
}: {
  citation: CitationResolution;
  onDismiss: () => void;
}) {
  return (
    <aside aria-label="Search result location" className="note-search-evidence" role="status">
      <div>
        <strong>{citationOwner(citation)}</strong>
        <span>{citationLocation(citation)}</span>
        <q>{citation.excerpt}</q>
      </div>
      <button aria-label="Dismiss search result location" onClick={onDismiss} type="button">
        ×
      </button>
    </aside>
  );
}

function SearchIntegrityNotice({
  focus,
  onDismiss,
}: {
  focus: Extract<SearchFocusState, { phase: "HISTORICAL" | "UNAVAILABLE" }>;
  onDismiss: () => void;
}) {
  return (
    <aside aria-label="Search citation warning" className="note-search-warning" role="alert">
      <div>
        <strong>{focus.phase === "HISTORICAL" ? "Older revision" : "Match unavailable"}</strong>
        <span>{focus.message}</span>
        {focus.citation && <q>{focus.citation.excerpt}</q>}
      </div>
      <button aria-label="Dismiss search citation warning" onClick={onDismiss} type="button">
        ×
      </button>
    </aside>
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
    bodyMarkdown: note.bodyMarkdown,
    sources: note.sources.map(({ label, url }) => ({ label, url })),
  });
}

async function loadNoteSession(
  backend: Backend,
  mode: NotePageProps["mode"],
  noteId: string,
): Promise<NoteSession> {
  await waitForWorkingCopyReconciliation(backend, noteId);
  if (mode === "ephemeral") {
    const workingCopy = await backend.loadWorkingCopy(noteId);
    return {
      coordinator: workingCopy
        ? NoteAutosaveCoordinator.recovered(backend, workingCopy)
        : NoteAutosaveCoordinator.ephemeral(backend, { noteId }),
      noteId,
      note: null,
    };
  }
  const [note, workingCopy] = await Promise.all([
    backend.loadTidbit(noteId),
    backend.loadWorkingCopy(noteId),
  ]);
  if (note.deletedAtMs !== null) throw new DeletedNoteError();
  return {
    coordinator: coordinatorForDurableNote(backend, note, workingCopy),
    noteId,
    note,
  };
}

class DeletedNoteError extends Error {}

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

function scheduleWorkingCopyReconciliation(backend: Backend): void {
  if (reconciliationStarted.has(backend)) return;
  reconciliationStarted.add(backend);
  window.setTimeout(() => {
    void backend
      .listWorkingCopies()
      .then(async (workingCopies) => {
        for (const workingCopy of workingCopies) {
          if (workingCopy.noteId === activeNoteIds.get(backend)) continue;
          const operation = reconcileWorkingCopy(backend, workingCopy);
          const operations = operationsFor(backend);
          operations.set(workingCopy.noteId, operation);
          try {
            await operation;
          } catch (reason) {
            console.warn(`Could not reconcile interrupted note ${workingCopy.noteId}`, reason);
          } finally {
            if (operations.get(workingCopy.noteId) === operation) {
              operations.delete(workingCopy.noteId);
            }
          }
        }
      })
      .catch((reason: unknown) => {
        console.error("Could not reconcile interrupted note autosaves", reason);
      });
  }, 0);
}

async function reconcileWorkingCopy(backend: Backend, workingCopy: WorkingCopyRecord) {
  if (workingCopy.noteId === activeNoteIds.get(backend)) return;
  if (workingCopy.mediaReservation) {
    await backend.discardWorkingCopy({
      noteId: workingCopy.noteId,
      expectedEditGeneration: workingCopy.editGeneration,
    });
    return;
  }
  const save = await backend.saveWorkingCopy({
    noteId: workingCopy.noteId,
    baseRevisionId: workingCopy.baseRevisionId,
    editGeneration: workingCopy.editGeneration + 1,
    bodyMarkdown: workingCopy.bodyMarkdown,
    sources: workingCopy.sources,
  });
  if (save.status !== "SAVED" || workingCopy.noteId === activeNoteIds.get(backend)) return;
  await backend.checkpointWorkingCopy({
    noteId: workingCopy.noteId,
    expectedEditGeneration: save.acceptedEditGeneration,
  });
}

function operationsFor(backend: Backend): Map<string, Promise<void>> {
  let operations = reconciliationOperations.get(backend);
  if (!operations) {
    operations = new Map();
    reconciliationOperations.set(backend, operations);
  }
  return operations;
}

async function waitForWorkingCopyReconciliation(backend: Backend, noteId: string): Promise<void> {
  await reconciliationOperations.get(backend)?.get(noteId);
}

function restoreScroll(noteId: string): void {
  const scrollTop = scrollPositions.get(noteId);
  if (scrollTop === undefined) return;
  window.scrollTo({ behavior: "instant", top: scrollTop });
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}
