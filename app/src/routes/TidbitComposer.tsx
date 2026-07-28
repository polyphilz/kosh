import { useBlocker } from "@tanstack/react-router";
import { listen } from "@tauri-apps/api/event";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
} from "react";
import { useBackend } from "../backend/context";
import type {
  DraftRecord,
  PdfRecord,
  SaveDraftInput,
  SourceDraft,
  TidbitDraft,
  TidbitRecord,
} from "../backend/contracts";
import { Button } from "../components/Button";
import { Dialog } from "../components/Dialog";
import { Input } from "../components/Input";
import { RichTextEditor, type RichTextEditorHandle } from "../markdown/RichTextEditor";

const AUTOSAVE_DELAY_MS = 350;
const IMAGE_DROP_EVENT = "kosh://image-drop";
const PDF_DROP_EVENT = "kosh://pdf-drop";

interface ImageDropNotice {
  dropId: string;
  filenames: string[];
}

interface PdfDropNotice {
  selections: Array<{
    selectionId: string;
    filename: string;
  }>;
}

interface ComposerState {
  title: string;
  bodyMarkdown: string;
  sources: EditableSource[];
}

interface EditableSource {
  key: number;
  label: string;
  url: string;
}

interface TidbitComposerProps {
  onCancel: () => void;
  onSaved: (tidbit: TidbitRecord) => void | Promise<void>;
  tidbit?: TidbitRecord;
}

type CancelIntent = "button" | null;
type DraftStatus = "idle" | "pending" | "saving" | "saved" | "failed";

export function TidbitComposer({ onCancel, onSaved, tidbit }: TidbitComposerProps) {
  const backend = useBackend();
  const contextKey = tidbit ? `edit:${tidbit.id}` : "capture";
  const nextSourceKey = useRef(1);
  const [state, setState] = useState<ComposerState>(() => initialState(tidbit, nextSourceKey));
  const [baseRevisionId, setBaseRevisionId] = useState<string | null>(
    tidbit?.currentRevisionId ?? null,
  );
  const [ready, setReady] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [cancelIntent, setCancelIntent] = useState<CancelIntent>(null);
  const [draftStatus, setDraftStatus] = useState<DraftStatus>("idle");
  const [error, setError] = useState<string | null>(null);
  const [recoveryAttempt, setRecoveryAttempt] = useState(0);
  const [editorMediaPending, setEditorMediaPending] = useState(false);
  const [dropMediaPending, setDropMediaPending] = useState(false);
  const dirtyRef = useRef(false);
  const busyRef = useRef(false);
  const editorMediaPendingRef = useRef(false);
  const mountedRef = useRef(true);
  const pendingDropCountRef = useRef(0);
  const queueRef = useRef<Promise<void>>(Promise.resolve());
  const editorRef = useRef<RichTextEditorHandle>(null);
  const stateRef = useRef(state);
  const readyRef = useRef(ready);

  stateRef.current = state;
  readyRef.current = ready;
  const mediaPending = editorMediaPending || dropMediaPending;
  const mediaIsPending = () => editorMediaPendingRef.current || pendingDropCountRef.current > 0;

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    setReady(false);
    setDraftStatus("idle");
    setError(null);
    void backend
      .loadDraft(contextKey)
      .then((draft) => {
        if (!active) return;
        if (draft) {
          setState(stateFromDraft(draft, nextSourceKey));
          setBaseRevisionId(draft.baseRevisionId);
          dirtyRef.current = true;
          setDirty(true);
          setDraftStatus("saved");
        }
        setReady(true);
      })
      .catch((reason: unknown) => {
        if (!active) return;
        setDraftStatus("failed");
        setError(`Draft recovery failed: ${errorMessage(reason)}`);
      });
    return () => {
      active = false;
    };
  }, [backend, contextKey, recoveryAttempt]);

  const draftInput = useCallback(
    (snapshot: ComposerState): SaveDraftInput => ({
      contextKey,
      tidbitId: tidbit?.id ?? null,
      baseRevisionId,
      ...committableDraft(snapshot, false),
    }),
    [baseRevisionId, contextKey, tidbit?.id],
  );

  const enqueueDraftSave = useCallback(
    (snapshot: ComposerState) => {
      const operation = queueRef.current
        .catch(() => undefined)
        .then(() => backend.saveDraft(draftInput(snapshot)));
      queueRef.current = operation.then(
        () => undefined,
        () => undefined,
      );
      return operation;
    },
    [backend, draftInput],
  );

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) {
      return;
    }
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen<ImageDropNotice>(IMAGE_DROP_EVENT, (event) => {
      if (!active || !readyRef.current || busyRef.current) {
        return;
      }
      pendingDropCountRef.current += 1;
      setDropMediaPending(true);
      setError(null);
      void enqueueDraftSave(stateRef.current)
        .then((draft) => backend.ingestDroppedImages(event.payload.dropId, draft.id))
        .then((result) => {
          if (!active) {
            return;
          }
          editorRef.current?.insertImages(result.images);
          if (result.failures.length > 0) {
            setError(
              `Could not add ${result.failures.map((failure) => failure.filename).join(", ")}: ${result.failures.map((failure) => failure.message).join("; ")}`,
            );
          }
        })
        .catch((reason: unknown) => {
          if (active) {
            setError(`Could not add dropped image: ${errorMessage(reason)}`);
          }
        })
        .finally(() => {
          pendingDropCountRef.current = Math.max(0, pendingDropCountRef.current - 1);
          if (active) {
            setDropMediaPending(pendingDropCountRef.current > 0);
          }
        });
    }).then((stop) => {
      if (active) {
        unlisten = stop;
      } else {
        stop();
      }
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [backend, enqueueDraftSave]);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) {
      return;
    }
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen<PdfDropNotice>(PDF_DROP_EVENT, (event) => {
      if (!active || !readyRef.current || busyRef.current) {
        return;
      }
      pendingDropCountRef.current += 1;
      setDropMediaPending(true);
      setError(null);
      void enqueueDraftSave(stateRef.current)
        .then(async (draft) => {
          const pdfs: PdfRecord[] = [];
          const failures: string[] = [];
          for (const selection of event.payload.selections) {
            try {
              pdfs.push(await backend.ingestSelectedPdf(selection.selectionId, draft.id));
            } catch (reason) {
              failures.push(`${selection.filename}: ${errorMessage(reason)}`);
            }
          }
          return { failures, pdfs };
        })
        .then(({ failures, pdfs }) => {
          if (!active) return;
          editorRef.current?.insertPdfs(pdfs);
          if (failures.length > 0) {
            setError(`Could not add dropped PDFs: ${failures.join("; ")}`);
          }
        })
        .catch((reason: unknown) => {
          if (active) {
            setError(`Could not add dropped PDF: ${errorMessage(reason)}`);
          }
        })
        .finally(() => {
          pendingDropCountRef.current = Math.max(0, pendingDropCountRef.current - 1);
          if (active) {
            setDropMediaPending(pendingDropCountRef.current > 0);
          }
        });
    }).then((stop) => {
      if (active) {
        unlisten = stop;
      } else {
        stop();
      }
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [backend, enqueueDraftSave]);

  useEffect(() => {
    if (!ready || !dirty || busy) return;
    setDraftStatus("pending");
    const timer = window.setTimeout(() => {
      if (busyRef.current) return;
      setDraftStatus("saving");
      void enqueueDraftSave(state)
        .then(() => {
          if (mountedRef.current && !busyRef.current) setDraftStatus("saved");
        })
        .catch((reason: unknown) => {
          if (!mountedRef.current || busyRef.current) return;
          setDraftStatus("failed");
          setError(`Draft autosave failed: ${errorMessage(reason)}`);
        });
    }, AUTOSAVE_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [busy, dirty, enqueueDraftSave, ready, state]);

  const shouldBlock = useCallback(
    () =>
      (dirtyRef.current || editorMediaPendingRef.current || pendingDropCountRef.current > 0) &&
      !busyRef.current,
    [],
  );
  const blocker = useBlocker({
    disabled: !dirty && !mediaPending,
    enableBeforeUnload: dirty || mediaPending,
    shouldBlockFn: shouldBlock,
    withResolver: true,
  });

  const markChanged = (update: (current: ComposerState) => ComposerState) => {
    setState(update);
    dirtyRef.current = true;
    setDirty(true);
    setError(null);
  };

  const submit = async (event?: FormEvent<HTMLFormElement>) => {
    event?.preventDefault();
    if (busyRef.current || mediaIsPending()) return;
    busyRef.current = true;
    setBusy(true);
    setError(null);
    let savedDraft: DraftRecord;
    let savedTidbit: TidbitRecord;
    try {
      savedDraft = await enqueueDraftSave(state);
      const input = committableDraft(state, true);
      savedTidbit = tidbit
        ? await backend.editTidbit({
            ...input,
            id: tidbit.id,
            expectedRevisionId: baseRevisionId ?? tidbit.currentRevisionId,
          })
        : await backend.createTidbit(input);
    } catch (reason: unknown) {
      busyRef.current = false;
      setBusy(false);
      setError(saveErrorMessage(reason, Boolean(tidbit)));
      return;
    }

    try {
      const cleared = await backend.clearDraft({
        contextKey,
        expectedUpdatedAtMs: savedDraft.updatedAtMs,
      });
      if (!cleared) {
        console.warn("A newer draft was preserved after the tidbit commit");
      }
    } catch (reason: unknown) {
      console.warn("The committed tidbit's recovery draft could not be cleared", reason);
    }
    dirtyRef.current = false;
    setDirty(false);
    try {
      await onSaved(savedTidbit);
    } catch (reason: unknown) {
      setError(`The tidbit was saved, but its detail view could not open: ${errorMessage(reason)}`);
    }
  };

  const discardDraft = async () => {
    if (busyRef.current || mediaIsPending()) return false;
    busyRef.current = true;
    setBusy(true);
    setError(null);
    try {
      await queueRef.current;
      const latest = await backend.loadDraft(contextKey);
      if (
        latest &&
        !(await backend.clearDraft({
          contextKey,
          expectedUpdatedAtMs: latest.updatedAtMs,
        }))
      ) {
        throw new Error("a newer autosave arrived while discarding");
      }
      dirtyRef.current = false;
      setDirty(false);
      return true;
    } catch (reason: unknown) {
      busyRef.current = false;
      setBusy(false);
      setError(`Could not discard the draft: ${errorMessage(reason)}`);
      return false;
    }
  };

  const cancelWithoutChanges = () => {
    busyRef.current = true;
    onCancel();
  };

  const confirmDiscard = async () => {
    if (mediaIsPending()) return;
    const discarded = await discardDraft();
    if (!discarded) return;
    setCancelIntent(null);
    if (blocker.status === "blocked") {
      blocker.proceed();
    } else {
      onCancel();
    }
  };

  const discardDialogOpen = cancelIntent !== null || blocker.status === "blocked";

  return (
    <>
      <form
        className="capture-card"
        onKeyDown={(event: KeyboardEvent<HTMLFormElement>) => {
          if (
            event.key === "Enter" &&
            (event.metaKey || event.ctrlKey) &&
            !event.defaultPrevented &&
            !mediaIsPending()
          ) {
            event.preventDefault();
            event.currentTarget.requestSubmit();
          }
        }}
        onSubmit={(event) => void submit(event)}
      >
        <div aria-live="polite" className="capture-card__status">
          {draftStatusLabel(draftStatus, ready)}
        </div>

        <label htmlFor={`${contextKey}-title`}>
          Title <span>optional</span>
        </label>
        <Input
          autoComplete="off"
          disabled={!ready || busy}
          id={`${contextKey}-title`}
          onChange={(event) =>
            markChanged((current) => ({ ...current, title: event.target.value }))
          }
          placeholder="A useful handle"
          value={state.title}
        />

        <label>Tidbit</label>
        <RichTextEditor
          ariaLabel="Tidbit"
          disabled={!ready || busy}
          imageStatus={(attachmentId) => backend.imageStatus(attachmentId)}
          onChange={(bodyMarkdown) => {
            if (bodyMarkdown !== stateRef.current.bodyMarkdown) {
              markChanged((current) => ({ ...current, bodyMarkdown }));
            }
          }}
          onImageError={(reason) => setError(`Could not add attachment: ${errorMessage(reason)}`)}
          onPendingImagesChange={(pending) => {
            editorMediaPendingRef.current = pending;
            setEditorMediaPending(pending);
          }}
          pasteImage={async () => {
            const captureId = await backend.captureClipboardImage();
            const draft = await enqueueDraftSave(stateRef.current);
            return backend.ingestClipboardImage(captureId, draft.id);
          }}
          pickImage={async () => {
            const selectionId = await backend.selectImage();
            if (!selectionId) return null;
            const draft = await enqueueDraftSave(stateRef.current);
            return backend.ingestSelectedImage(selectionId, draft.id);
          }}
          pdfStatus={(attachmentId) => backend.pdfStatus(attachmentId)}
          openPdfExternal={(attachmentId) => backend.openPdfExternal(attachmentId)}
          pickPdf={async () => {
            const selectionId = await backend.selectPdf();
            if (!selectionId) return null;
            const draft = await enqueueDraftSave(stateRef.current);
            return backend.ingestSelectedPdf(selectionId, draft.id);
          }}
          placeholder="Drop the knowledge here…"
          ref={editorRef}
          retryImageOcr={(attachmentId) => backend.retryImageOcr(attachmentId)}
          retryPdfExtraction={(attachmentId) => backend.retryPdfExtraction(attachmentId)}
          value={state.bodyMarkdown}
        />

        <fieldset className="source-fields">
          <legend>
            Sources <span>optional</span>
          </legend>
          {state.sources.map((source, position) => (
            <div className="source-fields__row" key={source.key}>
              <label>
                <span className="visually-hidden">Source {position + 1} label</span>
                <Input
                  aria-label={`Source ${position + 1} label`}
                  disabled={!ready || busy}
                  onChange={(event) => {
                    const label = event.target.value;
                    markChanged((current) => ({
                      ...current,
                      sources: current.sources.map((candidate) =>
                        candidate.key === source.key ? { ...candidate, label } : candidate,
                      ),
                    }));
                  }}
                  placeholder="Label"
                  value={source.label}
                />
              </label>
              <label>
                <span className="visually-hidden">Source {position + 1} URL</span>
                <Input
                  aria-label={`Source ${position + 1} URL`}
                  disabled={!ready || busy}
                  onChange={(event) => {
                    const url = event.target.value;
                    markChanged((current) => ({
                      ...current,
                      sources: current.sources.map((candidate) =>
                        candidate.key === source.key ? { ...candidate, url } : candidate,
                      ),
                    }));
                  }}
                  placeholder="https://…"
                  type="url"
                  value={source.url}
                />
              </label>
              <Button
                aria-label={`Remove source ${position + 1}`}
                disabled={!ready || busy}
                onClick={() =>
                  markChanged((current) => ({
                    ...current,
                    sources: current.sources.filter((candidate) => candidate.key !== source.key),
                  }))
                }
                size="compact"
                variant="ghost"
              >
                Remove
              </Button>
            </div>
          ))}
          <Button
            disabled={!ready || busy}
            onClick={() =>
              markChanged((current) => ({
                ...current,
                sources: [...current.sources, { key: nextSourceKey.current++, label: "", url: "" }],
              }))
            }
            size="compact"
            variant="ghost"
          >
            Add source
          </Button>
        </fieldset>

        {error && (
          <div className="capture-card__error" role="alert">
            <p>{error}</p>
            {!ready && draftStatus === "failed" && (
              <Button
                onClick={() => setRecoveryAttempt((attempt) => attempt + 1)}
                size="compact"
                variant="ghost"
              >
                Retry draft recovery
              </Button>
            )}
          </div>
        )}

        <footer>
          <Button
            disabled={!ready || busy || mediaPending}
            onClick={() => {
              if (dirtyRef.current || mediaIsPending()) setCancelIntent("button");
              else cancelWithoutChanges();
            }}
            variant="ghost"
          >
            Cancel
          </Button>
          <Button
            aria-keyshortcuts="Meta+Enter Control+Enter"
            disabled={!ready || busy || mediaPending || !state.bodyMarkdown.trim()}
            type="submit"
            variant="accent"
          >
            {mediaPending
              ? "Adding attachment…"
              : busy
                ? "Saving…"
                : tidbit
                  ? "Save changes"
                  : "Save tidbit"}
          </Button>
        </footer>
      </form>

      <Dialog
        description="This removes the local recovery copy."
        footer={
          <>
            <Button
              data-autofocus
              disabled={busy}
              onClick={() => {
                setCancelIntent(null);
                if (blocker.status === "blocked") blocker.reset();
              }}
              variant="ghost"
            >
              Keep editing
            </Button>
            <Button
              disabled={busy || mediaPending}
              onClick={() => void confirmDiscard()}
              variant="danger"
            >
              {mediaPending ? "Adding attachment…" : busy ? "Discarding…" : "Discard draft"}
            </Button>
          </>
        }
        onClose={() => {
          if (busy) return;
          setCancelIntent(null);
          if (blocker.status === "blocked") blocker.reset();
        }}
        open={discardDialogOpen}
        title="Discard this draft?"
      >
        <p>
          {mediaPending
            ? "Wait for pending attachments to finish before discarding this draft."
            : "Your changes cannot be recovered after they are discarded."}
        </p>
      </Dialog>
    </>
  );
}

function initialState(tidbit: TidbitRecord | undefined, key: { current: number }): ComposerState {
  if (!tidbit) {
    return { title: "", bodyMarkdown: "", sources: [] };
  }
  return {
    title: tidbit.title ?? "",
    bodyMarkdown: tidbit.bodyMarkdown,
    sources: tidbit.sources.map((source) => ({
      key: key.current++,
      label: source.label ?? "",
      url: source.url ?? "",
    })),
  };
}

function stateFromDraft(draft: DraftRecord, key: { current: number }): ComposerState {
  return {
    title: draft.title ?? "",
    bodyMarkdown: draft.bodyMarkdown,
    sources: draft.sources.map((source) => ({
      key: key.current++,
      label: source.label ?? "",
      url: source.url ?? "",
    })),
  };
}

function committableDraft(state: ComposerState, dropEmptySources: boolean): TidbitDraft {
  const sources: SourceDraft[] = state.sources
    .filter((source) => !dropEmptySources || source.label.trim() || source.url.trim())
    .map((source) => ({
      label: source.label || null,
      url: source.url || null,
    }));
  return {
    title: state.title || null,
    bodyMarkdown: state.bodyMarkdown,
    sources,
  };
}

function draftStatusLabel(status: DraftStatus, ready: boolean): string {
  if (!ready) return "Checking for a recovered draft…";
  switch (status) {
    case "pending":
      return "Draft changed";
    case "saving":
      return "Saving draft…";
    case "saved":
      return "Draft saved locally";
    case "failed":
      return "Draft recovery needs attention";
    default:
      return "Draft stays local";
  }
}

function saveErrorMessage(reason: unknown, editing: boolean): string {
  const message = errorMessage(reason);
  if (editing && /stale|revision/i.test(message)) {
    return "This tidbit changed elsewhere. Your draft is safe; reopen the tidbit before retrying.";
  }
  return `Could not save the tidbit: ${message}`;
}

function errorMessage(reason: unknown): string {
  if (reason instanceof Error) return reason.message;
  if (
    reason &&
    typeof reason === "object" &&
    "message" in reason &&
    typeof reason.message === "string"
  ) {
    return reason.message;
  }
  return String(reason);
}
