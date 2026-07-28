import { useBlocker } from "@tanstack/react-router";
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
  SaveDraftInput,
  SourceDraft,
  TidbitDraft,
  TidbitRecord,
} from "../backend/contracts";
import { Button } from "../components/Button";
import { Dialog } from "../components/Dialog";
import { Input } from "../components/Input";
import { RichTextEditor } from "../markdown/RichTextEditor";

const AUTOSAVE_DELAY_MS = 350;

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
  const dirtyRef = useRef(false);
  const busyRef = useRef(false);
  const mountedRef = useRef(true);
  const queueRef = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    setReady(false);
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
      })
      .catch((reason: unknown) => {
        if (!active) return;
        setDraftStatus("failed");
        setError(`Draft recovery failed: ${errorMessage(reason)}`);
      })
      .finally(() => {
        if (active) setReady(true);
      });
    return () => {
      active = false;
    };
  }, [backend, contextKey]);

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

  const shouldBlock = useCallback(() => dirtyRef.current && !busyRef.current, []);
  const blocker = useBlocker({
    disabled: !dirty,
    enableBeforeUnload: dirty,
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
    if (busyRef.current) return;
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
    if (busyRef.current) return false;
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
            !event.defaultPrevented
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
          onChange={(bodyMarkdown) => markChanged((current) => ({ ...current, bodyMarkdown }))}
          placeholder="Drop the knowledge here…"
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
          <p className="capture-card__error" role="alert">
            {error}
          </p>
        )}

        <footer>
          <Button
            disabled={!ready || busy}
            onClick={() => {
              if (dirtyRef.current) setCancelIntent("button");
              else cancelWithoutChanges();
            }}
            variant="ghost"
          >
            Cancel
          </Button>
          <Button
            aria-keyshortcuts="Meta+Enter Control+Enter"
            disabled={!ready || busy || !state.bodyMarkdown.trim()}
            type="submit"
            variant="accent"
          >
            {busy ? "Saving…" : tidbit ? "Save changes" : "Save tidbit"}
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
            <Button disabled={busy} onClick={() => void confirmDiscard()} variant="danger">
              {busy ? "Discarding…" : "Discard draft"}
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
        <p>Your changes cannot be recovered after they are discarded.</p>
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
