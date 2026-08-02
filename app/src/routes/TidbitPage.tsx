import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { useBackend } from "../backend/context";
import {
  TIDBIT_PURGE_DELAY_MS,
  type CitationResolution,
  type TidbitRecord,
  type TidbitRevisionAttachment,
  type TidbitRevisionRecord,
  type TidbitRevisionSummary,
  type TidbitSource,
} from "../backend/contracts";
import { Button } from "../components/Button";
import { Dialog } from "../components/Dialog";
import { ErrorState, LoadingState } from "../components/States";
import { Status } from "../components/Status";
import { useDeadlineReached } from "../hooks/useDeadlineReached";
import { attachmentMediaUrl } from "../media/gateway";
import { MarkdownRenderer } from "../markdown/MarkdownRenderer";
import { markdownToPlainText } from "../markdown/plainText";
import { citationLocation } from "../search/presentation";
import { TidbitComposer } from "./TidbitComposer";

const HISTORY_PAGE_SIZE = 20;

export function TidbitPage() {
  const backend = useBackend();
  const navigate = useNavigate();
  const { tidbitId } = useParams({ from: "/tidbits/$tidbitId" });
  const route = useSearch({ from: "/tidbits/$tidbitId" });
  const citationRef = useRef<HTMLElement>(null);
  const [tidbit, setTidbit] = useState<TidbitRecord | null>(null);
  const [revision, setRevision] = useState<TidbitRevisionRecord | null>(null);
  const [history, setHistory] = useState<TidbitRevisionSummary[]>([]);
  const [historyCursor, setHistoryCursor] = useState<number | null>(null);
  const [citation, setCitation] = useState<CitationResolution | null>(null);
  const [citationRefresh, setCitationRefresh] = useState(0);
  const [citationLoading, setCitationLoading] = useState(false);
  const [citationError, setCitationError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionStatus, setActionStatus] = useState("");
  const [editing, setEditing] = useState(false);
  const [working, setWorking] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [purgeOpen, setPurgeOpen] = useState(false);
  const purgeEligibleAt =
    tidbit?.deletedAtMs == null ? null : tidbit.deletedAtMs + TIDBIT_PURGE_DELAY_MS;
  const purgeEligible = useDeadlineReached(purgeEligibleAt);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setLoadError(null);
    setRevision(null);
    void backend
      .loadTidbit(tidbitId)
      .then(async (loaded) => {
        const selectedRevisionId = route.revision ?? loaded.currentRevisionId;
        const selected = await backend.loadTidbitRevision(tidbitId, selectedRevisionId);
        if (!active) return;
        setTidbit(loaded);
        setRevision(selected);
      })
      .catch((reason: unknown) => {
        if (active) setLoadError(errorMessage(reason));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [backend, route.revision, tidbitId]);

  useEffect(() => {
    let active = true;
    setHistory([]);
    setHistoryCursor(null);
    setHistoryLoading(true);
    void backend
      .listTidbitRevisions({
        beforeRevisionNumber: null,
        limit: HISTORY_PAGE_SIZE,
        tidbitId,
      })
      .then((page) => {
        if (!active) return;
        setHistory(page.items);
        setHistoryCursor(page.nextBeforeRevisionNumber);
      })
      .catch((reason: unknown) => {
        if (active) setActionError(`Could not load revision history: ${errorMessage(reason)}`);
      })
      .finally(() => {
        if (active) setHistoryLoading(false);
      });
    return () => {
      active = false;
    };
  }, [backend, citationRefresh, tidbitId]);

  useEffect(() => {
    let active = true;
    setCitation(null);
    setCitationError(null);
    if (!route.passage) {
      setCitationLoading(false);
      return;
    }
    setCitationLoading(true);
    void backend
      .resolveCitation(route.passage)
      .then((resolved) => {
        if (!active) return;
        if (resolved.tidbit?.id !== tidbitId) {
          throw new Error("The citation does not belong to this tidbit.");
        }
        setCitation(resolved);
      })
      .catch((reason: unknown) => {
        if (active) setCitationError(errorMessage(reason));
      })
      .finally(() => {
        if (active) setCitationLoading(false);
      });
    return () => {
      active = false;
    };
  }, [backend, citationRefresh, route.passage, tidbitId]);

  useEffect(() => {
    if (!citation || !route.passage) return;
    const frame = window.requestAnimationFrame(() => citationRef.current?.focus());
    return () => window.cancelAnimationFrame(frame);
  }, [citation, route.passage]);

  if (loading) {
    return (
      <main className="page page--narrow">
        <LoadingState detail="Opening the requested immutable revision…" title="Loading tidbit" />
      </main>
    );
  }
  if (loadError || !tidbit || !revision) {
    return (
      <main className="page page--narrow">
        <ErrorState
          action={<BackLink exact={route.exact} from={route.from} q={route.q} view={route.view} />}
          detail={loadError ?? "The tidbit or revision was not found."}
          title="Could not open tidbit"
        />
      </main>
    );
  }

  if (editing) {
    return (
      <main className="page page--narrow">
        <header className="page-header">
          <div>
            <p className="page-kicker">Revision {tidbit.revisionNumber}</p>
            <h1>Edit tidbit</h1>
            <p>Your recovery draft remains tied to this exact revision.</p>
          </div>
          <Status>Draft stays local</Status>
        </header>
        <TidbitComposer
          key={tidbit.currentRevisionId}
          onCancel={() => setEditing(false)}
          onSaved={(saved) => {
            setTidbit(saved);
            setCitationRefresh((value) => value + 1);
            setEditing(false);
            void backend
              .loadTidbitRevision(saved.id, saved.currentRevisionId)
              .then(setRevision)
              .catch((reason: unknown) =>
                setActionError(`Could not open the saved revision: ${errorMessage(reason)}`),
              );
            void navigate({
              params: { tidbitId },
              replace: true,
              search: {
                exact: route.exact,
                from: route.from,
                passage: route.passage,
                q: route.q,
                revision: undefined,
                view: route.view,
              },
              to: "/tidbits/$tidbitId",
            });
          }}
          tidbit={tidbit}
        />
      </main>
    );
  }

  const isCurrentRevision = revision.id === tidbit.currentRevisionId;

  const setSelectedRevision = (revisionId: string | undefined) => {
    void navigate({
      params: { tidbitId },
      search: {
        exact: route.exact,
        from: route.from,
        passage: undefined,
        q: route.q,
        revision: revisionId,
        view: route.view,
      },
      to: "/tidbits/$tidbitId",
    });
  };

  const copyText = (value: string, success: string) => {
    setActionError(null);
    setActionStatus("");
    if (!navigator.clipboard) {
      setActionError("Clipboard access is unavailable.");
      return;
    }
    void navigator.clipboard
      .writeText(value)
      .then(() => setActionStatus(success))
      .catch((reason: unknown) => setActionError(`Could not copy: ${errorMessage(reason)}`));
  };

  return (
    <main className="page page--narrow tidbit-page">
      <div className="tidbit-page__back">
        <BackLink exact={route.exact} from={route.from} q={route.q} view={route.view} />
      </div>
      <header className="page-header tidbit-page__header">
        <div>
          <p className="page-kicker">
            Revision {revision.revisionNumber}
            {!isCurrentRevision && " · Historical"}
          </p>
          <h1>{revision.displayTitle}</h1>
          <p>
            {isCurrentRevision ? "Updated" : "Created"}{" "}
            {new Date(
              isCurrentRevision ? tidbit.updatedAtMs : revision.createdAtMs,
            ).toLocaleString()}
          </p>
        </div>
        <div className="tidbit-page__actions">
          <Button
            onClick={() => copyText(revision.bodyMarkdown, "Markdown copied")}
            variant="surface"
          >
            Copy Markdown
          </Button>
          <Button
            onClick={() =>
              copyText(markdownToPlainText(revision.bodyMarkdown), "Plain text copied")
            }
            variant="surface"
          >
            Copy text
          </Button>
          {tidbit.deletedAtMs === null && isCurrentRevision && (
            <>
              <Button onClick={() => setEditing(true)} variant="primary">
                Edit
              </Button>
              <Button onClick={() => setDeleteOpen(true)} variant="danger">
                Delete
              </Button>
            </>
          )}
          {tidbit.deletedAtMs !== null && (
            <>
              <Button
                disabled={working}
                onClick={() => {
                  setWorking(true);
                  setActionError(null);
                  void backend
                    .restoreTidbit({
                      expectedRevisionId: tidbit.currentRevisionId,
                      id: tidbit.id,
                    })
                    .then((restored) => {
                      setTidbit(restored);
                      setRevision((current) =>
                        current ? { ...current, tidbitDeleted: false } : current,
                      );
                      setActionStatus("Tidbit restored");
                    })
                    .catch((reason: unknown) =>
                      setActionError(`Could not restore the tidbit: ${errorMessage(reason)}`),
                    )
                    .finally(() => setWorking(false));
                }}
                variant="primary"
              >
                {working ? "Restoring…" : "Restore"}
              </Button>
              <Button
                disabled={!purgeEligible || working}
                onClick={() => setPurgeOpen(true)}
                title={
                  purgeEligibleAt && !purgeEligible
                    ? `Available ${new Date(purgeEligibleAt).toLocaleString()}`
                    : undefined
                }
                variant="danger"
              >
                Delete permanently
              </Button>
            </>
          )}
        </div>
      </header>

      {tidbit.deletedAtMs !== null && (
        <section className="tidbit-page__trash-notice" role="status">
          <div>
            <strong>This tidbit is in Trash.</strong>
            <span>It is excluded from search and can still be restored.</span>
          </div>
          <Status tone={purgeEligible ? "danger" : "warning"}>
            {purgeEligible
              ? "Permanent delete available"
              : `Protected until ${new Date(purgeEligibleAt!).toLocaleDateString()}`}
          </Status>
        </section>
      )}

      {!isCurrentRevision && (
        <section className="tidbit-page__history-notice" role="status">
          <span>You are viewing immutable revision {revision.revisionNumber}.</span>
          <Button onClick={() => setSelectedRevision(undefined)} size="compact" variant="ghost">
            Return to current
          </Button>
        </section>
      )}

      {route.passage && citationLoading && (
        <p className="tidbit-citation-focus__loading" role="status">
          Resolving cited passage…
        </p>
      )}
      {citationError && (
        <p className="capture-card__error" role="alert">
          Could not open the cited passage: {citationError}
        </p>
      )}
      {citation && (
        <section
          aria-labelledby="tidbit-citation-title"
          className="tidbit-citation-focus"
          ref={citationRef}
          tabIndex={-1}
        >
          <header>
            <div>
              <p className="page-kicker">Cited passage</p>
              <h2 id="tidbit-citation-title">
                {citation.headingContext.at(-1) ?? citation.tidbit?.displayTitle ?? "Passage"}
              </h2>
            </div>
            <Status tone={citation.state === "CURRENT" ? "success" : "warning"}>
              {citation.state === "CURRENT" ? "Current revision" : "Historical revision"}
            </Status>
          </header>
          <p>{citationLocation(citation)}</p>
          <blockquote>{citation.excerpt}</blockquote>
          {citation.state === "HISTORICAL" && citation.tidbit && (
            <Button
              onClick={() => setSelectedRevision(citation.tidbit?.revisionId)}
              size="compact"
              variant="ghost"
            >
              Open cited revision {citation.tidbit.revisionNumber}
            </Button>
          )}
        </section>
      )}

      <article className="tidbit-page__content">
        <MarkdownRenderer source={revision.bodyMarkdown} />
      </article>

      <p aria-live="polite" className="tidbit-page__action-status" role="status">
        {actionStatus}
      </p>
      {actionError && (
        <p className="capture-card__error" role="alert">
          {actionError}
        </p>
      )}

      {revision.sources.length > 0 && (
        <SourcesSection
          onCopy={(source) => copyText(source.url ?? source.label ?? "", "Source copied")}
          onOpen={(source) => {
            setActionError(null);
            void backend
              .openSourceUrl(source.id)
              .catch((reason: unknown) =>
                setActionError(`Could not open the source: ${errorMessage(reason)}`),
              );
          }}
          sources={revision.sources}
        />
      )}

      {revision.attachments.length > 0 && (
        <AttachmentsSection
          attachments={revision.attachments}
          onError={(message) => setActionError(message)}
        />
      )}

      <section aria-labelledby="tidbit-history" className="tidbit-page__history">
        <div className="tidbit-page__section-header">
          <div>
            <p className="page-kicker">Immutable record</p>
            <h2 id="tidbit-history">Revision history</h2>
          </div>
          <span>{history.length} loaded</span>
        </div>
        {historyLoading && history.length === 0 ? (
          <LoadingState detail="Reading revision history…" title="Loading history" />
        ) : (
          <ol>
            {history.map((item) => (
              <li key={item.id}>
                <button
                  aria-current={item.id === revision.id ? "true" : undefined}
                  onClick={() => setSelectedRevision(item.isCurrent ? undefined : item.id)}
                  type="button"
                >
                  <span>
                    Revision {item.revisionNumber}
                    {item.isCurrent && " · Current"}
                  </span>
                  <strong>{item.displayTitle}</strong>
                  <small>
                    {new Date(item.createdAtMs).toLocaleString()} · {item.sourceCount} sources ·{" "}
                    {item.attachmentCount} attachments
                  </small>
                </button>
              </li>
            ))}
          </ol>
        )}
        {historyCursor !== null && (
          <Button
            disabled={historyLoading}
            onClick={() => {
              setHistoryLoading(true);
              void backend
                .listTidbitRevisions({
                  beforeRevisionNumber: historyCursor,
                  limit: HISTORY_PAGE_SIZE,
                  tidbitId,
                })
                .then((page) => {
                  setHistory((current) => [...current, ...page.items]);
                  setHistoryCursor(page.nextBeforeRevisionNumber);
                })
                .catch((reason: unknown) =>
                  setActionError(`Could not load more history: ${errorMessage(reason)}`),
                )
                .finally(() => setHistoryLoading(false));
            }}
            variant="surface"
          >
            {historyLoading ? "Loading…" : "Load older revisions"}
          </Button>
        )}
      </section>

      <Dialog
        description="The tidbit moves to Trash and leaves search immediately."
        footer={
          <>
            <Button
              data-autofocus
              disabled={working}
              onClick={() => setDeleteOpen(false)}
              variant="ghost"
            >
              Keep tidbit
            </Button>
            <Button
              disabled={working}
              onClick={() => {
                setWorking(true);
                setActionError(null);
                void backend
                  .deleteTidbit({
                    expectedRevisionId: tidbit.currentRevisionId,
                    id: tidbit.id,
                  })
                  .then(() => navigate({ search: { view: "trash" }, to: "/library" }))
                  .catch((reason: unknown) => {
                    setDeleteOpen(false);
                    setActionError(`Could not delete the tidbit: ${errorMessage(reason)}`);
                  })
                  .finally(() => setWorking(false));
              }}
              variant="danger"
            >
              {working ? "Deleting…" : "Move to Trash"}
            </Button>
          </>
        }
        onClose={() => {
          if (!working) setDeleteOpen(false);
        }}
        open={deleteOpen}
        title="Move this tidbit to Trash?"
      >
        <p>You can restore it for 30 days before permanent deletion becomes available.</p>
      </Dialog>

      <Dialog
        description="This removes every authored revision and cannot be undone."
        footer={
          <>
            <Button
              data-autofocus
              disabled={working}
              onClick={() => setPurgeOpen(false)}
              variant="ghost"
            >
              Cancel
            </Button>
            <Button
              disabled={working || !purgeEligible}
              onClick={() => {
                setWorking(true);
                setActionError(null);
                void backend
                  .purgeTidbit({
                    expectedRevisionId: tidbit.currentRevisionId,
                    id: tidbit.id,
                  })
                  .then(() =>
                    navigate({ replace: true, search: { view: "trash" }, to: "/library" }),
                  )
                  .catch((reason: unknown) => {
                    setPurgeOpen(false);
                    setActionError(`Could not permanently delete: ${errorMessage(reason)}`);
                  })
                  .finally(() => setWorking(false));
              }}
              variant="danger"
            >
              {working ? "Deleting permanently…" : "Delete every revision"}
            </Button>
          </>
        }
        onClose={() => {
          if (!working) setPurgeOpen(false);
        }}
        open={purgeOpen}
        title="Permanently delete this tidbit?"
      >
        <p>
          Research answers keep their exact citation snapshots, but this tidbit, its sources, and
          its authored history will disappear from the library.
        </p>
      </Dialog>
    </main>
  );
}

function BackLink({
  exact,
  from,
  q,
  view,
}: {
  exact: true | undefined;
  from: "library" | "research" | "search" | undefined;
  q: string | undefined;
  view: "all" | "recent" | "trash" | undefined;
}) {
  if (from === "search") {
    return (
      <Link className="search-citation-detail__link" search={{ exact, q }} to="/search">
        ← Back to search
      </Link>
    );
  }
  if (from === "research") {
    return (
      <Link className="search-citation-detail__link" to="/research">
        ← Back to research
      </Link>
    );
  }
  return (
    <Link
      className="search-citation-detail__link"
      search={view && view !== "recent" ? { view } : {}}
      to="/library"
    >
      ← Back to library
    </Link>
  );
}

function SourcesSection({
  onCopy,
  onOpen,
  sources,
}: {
  onCopy: (source: TidbitSource) => void;
  onOpen: (source: TidbitSource) => void;
  sources: TidbitSource[];
}) {
  return (
    <section aria-labelledby="tidbit-sources" className="tidbit-page__sources">
      <h2 id="tidbit-sources">Sources</h2>
      <ol>
        {sources.map((source) => (
          <li key={source.id}>
            <div>
              <span>{source.label ?? source.url}</span>
              {source.label && source.url && <small>{source.url}</small>}
            </div>
            <div className="tidbit-page__source-actions">
              <Button onClick={() => onCopy(source)} size="compact" variant="ghost">
                Copy
              </Button>
              {source.url && (
                <Button onClick={() => onOpen(source)} size="compact" variant="surface">
                  Open
                </Button>
              )}
            </div>
          </li>
        ))}
      </ol>
    </section>
  );
}

function AttachmentsSection({
  attachments,
  onError,
}: {
  attachments: TidbitRevisionAttachment[];
  onError: (message: string) => void;
}) {
  const backend = useBackend();
  const openAttachment = (attachment: TidbitRevisionAttachment) => {
    const action =
      attachment.kind === "PDF"
        ? backend.openPdfExternal(attachment.id)
        : backend.openAttachmentExternal(attachment.id);
    void action.catch((reason: unknown) =>
      onError(`Could not open ${attachment.displayFilename}: ${errorMessage(reason)}`),
    );
  };
  return (
    <section aria-labelledby="tidbit-attachments" className="tidbit-page__attachments">
      <h2 id="tidbit-attachments">Attachments</h2>
      <ul>
        {attachments.map((attachment) => (
          <li key={attachment.id}>
            {attachment.kind === "IMAGE" && attachment.deletedAtMs === null && (
              <img
                alt={attachment.displayFilename}
                loading="lazy"
                src={attachmentMediaUrl(attachment.id)}
              />
            )}
            <div>
              <strong>{attachment.displayFilename}</strong>
              <small>
                {attachment.kind} · {formatBytes(attachment.byteLength)} ·{" "}
                {attachment.extractionState.toLowerCase().replaceAll("_", " ")}
              </small>
            </div>
            {attachment.kind !== "IMAGE" && attachment.deletedAtMs === null && (
              <Button onClick={() => openAttachment(attachment)} size="compact" variant="surface">
                Open
              </Button>
            )}
            {attachment.deletedAtMs !== null && <Status tone="warning">Unavailable</Status>}
          </li>
        ))}
      </ul>
    </section>
  );
}

function formatBytes(value: number): string {
  if (value < 1_024) return `${value} B`;
  if (value < 1_024 * 1_024) return `${(value / 1_024).toFixed(1)} KB`;
  return `${(value / (1_024 * 1_024)).toFixed(1)} MB`;
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
