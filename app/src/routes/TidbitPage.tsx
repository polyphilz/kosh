import { useNavigate, useParams } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { useBackend } from "../backend/context";
import type { TidbitRecord } from "../backend/contracts";
import { Button } from "../components/Button";
import { Dialog } from "../components/Dialog";
import { ErrorState, LoadingState } from "../components/States";
import { Status } from "../components/Status";
import { MarkdownRenderer } from "../markdown/MarkdownRenderer";
import { TidbitComposer } from "./TidbitComposer";

export function TidbitPage() {
  const backend = useBackend();
  const navigate = useNavigate();
  const { tidbitId } = useParams({ from: "/tidbits/$tidbitId" });
  const [tidbit, setTidbit] = useState<TidbitRecord | null>(null);
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
