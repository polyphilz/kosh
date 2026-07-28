import { useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { useBackend } from "../backend/context";
import type { CitationResolution, TidbitRecord } from "../backend/contracts";
import { Button } from "../components/Button";
import { Dialog } from "../components/Dialog";
import { ErrorState, LoadingState } from "../components/States";
import { Status } from "../components/Status";
import { MarkdownRenderer } from "../markdown/MarkdownRenderer";
import { citationLocation } from "../search/presentation";
import { TidbitComposer } from "./TidbitComposer";

export function TidbitPage() {
  const backend = useBackend();
  const navigate = useNavigate();
  const { tidbitId } = useParams({ from: "/tidbits/$tidbitId" });
  const { passage } = useSearch({ from: "/tidbits/$tidbitId" });
  const citationRef = useRef<HTMLElement>(null);
  const [tidbit, setTidbit] = useState<TidbitRecord | null>(null);
  const [citation, setCitation] = useState<CitationResolution | null>(null);
  const [citationRefresh, setCitationRefresh] = useState(0);
  const [citationLoading, setCitationLoading] = useState(false);
  const [citationError, setCitationError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setLoadError(null);
    void backend
      .loadTidbit(tidbitId)
      .then((loaded) => {
        if (active) setTidbit(loaded);
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
  }, [backend, tidbitId]);

  useEffect(() => {
    let active = true;
    setCitation(null);
    setCitationError(null);
    if (!passage) {
      setCitationLoading(false);
      return;
    }
    setCitationLoading(true);
    void backend
      .resolveCitation(passage)
      .then((resolved) => {
        if (!active) return;
        if (resolved.tidbit?.id !== tidbitId) {
          throw new Error("The citation does not belong to this tidbit.");
        }
        setCitation(resolved);
        window.requestAnimationFrame(() => citationRef.current?.focus());
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
  }, [backend, citationRefresh, passage, tidbitId]);

  if (loading) {
    return (
      <main className="page page--narrow">
        <LoadingState detail="Opening the latest revision…" title="Loading tidbit" />
      </main>
    );
  }
  if (loadError || !tidbit) {
    return (
      <main className="page page--narrow">
        <ErrorState
          detail={loadError ?? "The tidbit was not found."}
          title="Could not open tidbit"
        />
      </main>
    );
  }
  if (tidbit.deletedAtMs !== null) {
    return (
      <main className="page page--narrow">
        <ErrorState
          detail="This tidbit has been removed from the library."
          title="Tidbit deleted"
        />
        <Button onClick={() => void navigate({ to: "/" })} variant="ghost">
          Back to search
        </Button>
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
          }}
          tidbit={tidbit}
        />
      </main>
    );
  }

  return (
    <main className="page page--narrow tidbit-page">
      <header className="page-header tidbit-page__header">
        <div>
          <p className="page-kicker">Revision {tidbit.revisionNumber}</p>
          <h1>{tidbit.displayTitle}</h1>
          <p>Updated {new Date(tidbit.updatedAtMs).toLocaleString()}</p>
        </div>
        <div className="tidbit-page__actions">
          <Button onClick={() => setEditing(true)} variant="primary">
            Edit
          </Button>
          <Button onClick={() => setDeleteOpen(true)} variant="danger">
            Delete
          </Button>
        </div>
      </header>

      {passage && citationLoading && (
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
          {citation.state === "HISTORICAL" && (
            <small>
              This immutable excerpt came from revision {citation.tidbit?.revisionNumber}. The
              current note appears below for comparison.
            </small>
          )}
        </section>
      )}

      <article className="tidbit-page__content">
        <MarkdownRenderer source={tidbit.bodyMarkdown} />
      </article>

      {actionError && (
        <p className="capture-card__error" role="alert">
          {actionError}
        </p>
      )}

      {tidbit.sources.length > 0 && (
        <section aria-labelledby="tidbit-sources" className="tidbit-page__sources">
          <h2 id="tidbit-sources">Sources</h2>
          <ol>
            {tidbit.sources.map((source) => (
              <li key={source.id}>
                <span>{source.label ?? source.url}</span>
                {source.label && source.url && <small>{source.url}</small>}
              </li>
            ))}
          </ol>
        </section>
      )}

      <Dialog
        description="The history stays in the local database, but this tidbit leaves active views."
        footer={
          <>
            <Button
              data-autofocus
              disabled={deleting}
              onClick={() => setDeleteOpen(false)}
              variant="ghost"
            >
              Keep tidbit
            </Button>
            <Button
              disabled={deleting}
              onClick={() => {
                setDeleting(true);
                setActionError(null);
                void backend
                  .deleteTidbit({
                    id: tidbit.id,
                    expectedRevisionId: tidbit.currentRevisionId,
                  })
                  .then(() => navigate({ to: "/" }))
                  .catch((reason: unknown) => {
                    setDeleting(false);
                    setDeleteOpen(false);
                    setActionError(`Could not delete the tidbit: ${errorMessage(reason)}`);
                  });
              }}
              variant="danger"
            >
              {deleting ? "Deleting…" : "Delete tidbit"}
            </Button>
          </>
        }
        onClose={() => {
          if (!deleting) setDeleteOpen(false);
        }}
        open={deleteOpen}
        title="Delete this tidbit?"
      >
        <p>This is a soft delete and can be recovered from the database.</p>
      </Dialog>
    </main>
  );
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
