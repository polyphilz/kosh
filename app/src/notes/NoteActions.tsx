import { useEffect, useRef, useState } from "react";
import type { SourceDraft } from "../backend/contracts";
import { Button } from "../components/Button";
import { Dialog } from "../components/Dialog";
import { keyboardEventMatchesAccelerator } from "../shortcuts/localShortcuts";

interface NoteActionsProps {
  canEditSources: boolean;
  canDelete: boolean;
  deleteError: string | null;
  deleteShortcut?: string;
  deleting: boolean;
  disabled: boolean;
  onDelete: () => void;
  onSourcesChange: (sources: SourceDraft[]) => void;
  sources: SourceDraft[];
}

export function NoteActions({
  canEditSources,
  canDelete,
  deleteError,
  deleteShortcut,
  deleting,
  disabled,
  onDelete,
  onSourcesChange,
  sources,
}: NoteActionsProps) {
  const firstInput = useRef<HTMLInputElement>(null);
  const sourcesTrigger = useRef<HTMLButtonElement>(null);
  const [sourcesOpen, setSourcesOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [drafts, setDrafts] = useState<SourceDraft[]>(sources);
  const [sourceError, setSourceError] = useState<string | null>(null);

  useEffect(() => {
    if (!sourcesOpen) setDrafts(cloneSources(sources));
  }, [sources, sourcesOpen]);

  useEffect(() => {
    if (!deleteShortcut) return;
    const openDeleteDialog = (event: KeyboardEvent) => {
      if (
        !canDelete ||
        disabled ||
        deleting ||
        deleteOpen ||
        sourcesOpen ||
        event.isComposing ||
        event.repeat ||
        !keyboardEventMatchesAccelerator(event, deleteShortcut) ||
        document.querySelector('[aria-modal="true"]')
      ) {
        return;
      }
      event.preventDefault();
      event.stopImmediatePropagation();
      setDeleteOpen(true);
    };
    window.addEventListener("keydown", openDeleteDialog, true);
    return () => window.removeEventListener("keydown", openDeleteDialog, true);
  }, [canDelete, deleteOpen, deleteShortcut, deleting, disabled, sourcesOpen]);

  const openSources = () => {
    setDrafts(sources.length > 0 ? cloneSources(sources) : [{ label: null, url: null }]);
    setSourceError(null);
    setSourcesOpen(true);
    window.requestAnimationFrame(() => {
      if (
        document.activeElement === document.body ||
        document.activeElement === sourcesTrigger.current
      ) {
        firstInput.current?.focus();
      }
    });
  };
  const closeSources = () => {
    setSourcesOpen(false);
    window.requestAnimationFrame(() => sourcesTrigger.current?.focus());
  };
  const updateDrafts = (nextDrafts: SourceDraft[]) => {
    setDrafts(nextDrafts);
    const prepared = prepareSources(nextDrafts);
    setSourceError(prepared.error);
    if (!prepared.error) onSourcesChange(prepared.sources);
  };

  return (
    <div className="note-actions">
      <button
        aria-expanded={sourcesOpen}
        aria-haspopup="dialog"
        className="note-actions__button"
        disabled={disabled || !canEditSources}
        onClick={() => (sourcesOpen ? closeSources() : openSources())}
        ref={sourcesTrigger}
        title={canEditSources ? "Edit note sources" : "Write something before adding sources"}
        type="button"
      >
        <span aria-hidden="true">↗</span>
        <span>Sources{sources.length > 0 ? ` ${sources.length}` : ""}</span>
      </button>
      {sourcesOpen && (
        <section
          aria-label="Note sources"
          className="note-sources"
          onKeyDown={(event) => {
            if (event.key !== "Escape") return;
            event.preventDefault();
            event.stopPropagation();
            closeSources();
          }}
          role="dialog"
        >
          <header>
            <strong>Sources</strong>
            <button aria-label="Close sources" onClick={closeSources} type="button">
              ×
            </button>
          </header>
          <div className="note-sources__rows">
            {drafts.map((source, index) => (
              <div className="note-sources__row" key={index}>
                <label>
                  <span>Label</span>
                  <input
                    onChange={(event) =>
                      updateDrafts(
                        drafts.map((candidate, position) =>
                          position === index
                            ? { ...candidate, label: event.target.value }
                            : candidate,
                        ),
                      )
                    }
                    placeholder="Book, person, paper…"
                    ref={index === 0 ? firstInput : undefined}
                    value={source.label ?? ""}
                  />
                </label>
                <label>
                  <span>URL</span>
                  <input
                    inputMode="url"
                    onChange={(event) =>
                      updateDrafts(
                        drafts.map((candidate, position) =>
                          position === index
                            ? { ...candidate, url: event.target.value }
                            : candidate,
                        ),
                      )
                    }
                    placeholder="https://…"
                    value={source.url ?? ""}
                  />
                </label>
                <button
                  aria-label={`Remove source ${index + 1}`}
                  onClick={() => updateDrafts(drafts.filter((_, position) => position !== index))}
                  title="Remove source"
                  type="button"
                >
                  ×
                </button>
              </div>
            ))}
          </div>
          {sourceError && <p role="alert">{sourceError}</p>}
          <footer>
            <button
              onClick={() => updateDrafts([...drafts, { label: null, url: null }])}
              type="button"
            >
              + Add source
            </button>
            <span>Valid changes save with the note.</span>
          </footer>
        </section>
      )}

      <Dialog
        description="The note leaves search. You can undo immediately."
        footer={
          <>
            <Button disabled={deleting} onClick={() => setDeleteOpen(false)}>
              Cancel
            </Button>
            <Button
              className="note-delete-confirm"
              disabled={deleting}
              onClick={onDelete}
              variant="danger"
            >
              {deleting ? "Deleting…" : "Delete note"}
            </Button>
          </>
        }
        onClose={() => {
          if (!deleting) setDeleteOpen(false);
        }}
        initialFocus="panel"
        open={deleteOpen}
        title="Delete this note?"
      >
        <p className="note-delete-copy">Its revisions and attachments remain recoverable.</p>
        {deleteError && <p className="note-delete-error">{deleteError}</p>}
      </Dialog>
    </div>
  );
}

function prepareSources(drafts: readonly SourceDraft[]): {
  error: string | null;
  sources: SourceDraft[];
} {
  const sources: SourceDraft[] = [];
  const identities = new Set<string>();
  for (const draft of drafts) {
    const label = draft.label?.trim() || null;
    const rawUrl = draft.url?.trim() || null;
    if (!label && !rawUrl) continue;
    let url: string | null = null;
    if (rawUrl) {
      try {
        const parsed = new URL(rawUrl);
        if (!(["http:", "https:"] as string[]).includes(parsed.protocol) || !parsed.hostname) {
          return { error: "Source URLs must use HTTP or HTTPS.", sources: [] };
        }
        parsed.hash = "";
        url = parsed.toString();
      } catch {
        return { error: "Enter a complete HTTP or HTTPS URL.", sources: [] };
      }
    }
    const identity = `${label ?? ""}\u0000${url ?? ""}`;
    if (identities.has(identity)) {
      return { error: "Remove the duplicate source.", sources: [] };
    }
    identities.add(identity);
    sources.push({ label, url });
  }
  return { error: null, sources };
}

function cloneSources(sources: readonly SourceDraft[]): SourceDraft[] {
  return sources.map((source) => ({ ...source }));
}
