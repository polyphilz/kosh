import { Link, useSearch } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { useBackend } from "../backend/context";
import type { TidbitListCursor, TidbitListItem } from "../backend/contracts";
import { Button } from "../components/Button";
import { EmptyState, ErrorState, LoadingState } from "../components/States";
import { Status } from "../components/Status";

type LibraryView = "all" | "recent" | "trash";

const PAGE_SIZE = 30;
const RECENT_LIMIT = 12;

export function LibraryPage() {
  const backend = useBackend();
  const { view: routeView } = useSearch({ from: "/library" });
  const view: LibraryView = routeView ?? "recent";
  const [items, setItems] = useState<TidbitListItem[]>([]);
  const [cursor, setCursor] = useState<TidbitListCursor | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const requestGeneration = useRef(0);

  useEffect(() => {
    const generation = ++requestGeneration.current;
    let active = true;
    setItems([]);
    setCursor(null);
    setError(null);
    setLoading(true);
    setLoadingMore(false);
    void backend
      .listTidbits({
        cursor: null,
        limit: view === "recent" ? RECENT_LIMIT : PAGE_SIZE,
        scope: view === "trash" ? "DELETED" : "ACTIVE",
      })
      .then((page) => {
        if (!active || generation !== requestGeneration.current) return;
        setItems(page.items);
        setCursor(view === "recent" ? null : page.nextCursor);
      })
      .catch((reason: unknown) => {
        if (active && generation === requestGeneration.current) setError(errorMessage(reason));
      })
      .finally(() => {
        if (active && generation === requestGeneration.current) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [backend, reload, view]);

  const loadMore = () => {
    if (!cursor || loadingMore) return;
    const generation = requestGeneration.current;
    setLoadingMore(true);
    setError(null);
    void backend
      .listTidbits({
        cursor,
        limit: PAGE_SIZE,
        scope: view === "trash" ? "DELETED" : "ACTIVE",
      })
      .then((page) => {
        if (generation !== requestGeneration.current) return;
        setItems((current) => [...current, ...page.items]);
        setCursor(page.nextCursor);
      })
      .catch((reason: unknown) => {
        if (generation === requestGeneration.current) setError(errorMessage(reason));
      })
      .finally(() => {
        if (generation === requestGeneration.current) setLoadingMore(false);
      });
  };

  return (
    <main className="page library-page">
      <header className="page-header library-page__header">
        <div>
          <p className="page-kicker">Browse local knowledge</p>
          <h1>Library</h1>
          <p>Inspect every tidbit, immutable revision, source, and attachment.</p>
        </div>
        <Status tone={view === "trash" ? "warning" : "neutral"}>
          {view === "trash" ? "Recoverable trash" : "Local only"}
        </Status>
      </header>

      <nav aria-label="Library views" className="library-tabs">
        <LibraryTab label="Recent" selected={view === "recent"} view="recent" />
        <LibraryTab label="All tidbits" selected={view === "all"} view="all" />
        <LibraryTab label="Trash" selected={view === "trash"} view="trash" />
      </nav>

      {loading ? (
        <LoadingState detail="Reading the local library…" title="Loading tidbits" />
      ) : error && items.length === 0 ? (
        <ErrorState
          action={<Button onClick={() => setReload((value) => value + 1)}>Try again</Button>}
          detail={error}
          title="Could not load the library"
        />
      ) : items.length === 0 ? (
        <EmptyState
          action={
            view === "trash" ? null : (
              <Link className="search-citation-detail__link" to="/add">
                Add your first tidbit
              </Link>
            )
          }
          detail={
            view === "trash"
              ? "Soft-deleted tidbits will remain recoverable here for 30 days."
              : "Anything from a shower thought to chapter notes belongs here."
          }
          title={view === "trash" ? "Trash is empty" : "No tidbits yet"}
        />
      ) : (
        <>
          <ol
            aria-label={view === "trash" ? "Deleted tidbits" : "Tidbits"}
            className="library-list"
          >
            {items.map((item) => (
              <li key={item.id}>
                <Link
                  params={{ tidbitId: item.id }}
                  search={{ from: "library", passage: undefined, revision: undefined, view }}
                  to="/tidbits/$tidbitId"
                >
                  <div>
                    <h2>{item.displayTitle}</h2>
                    <p>{item.bodyPreview}</p>
                  </div>
                  <footer>
                    <span>
                      {view === "trash" ? "Deleted" : "Updated"}{" "}
                      {new Date(
                        view === "trash"
                          ? (item.deletedAtMs ?? item.updatedAtMs)
                          : item.updatedAtMs,
                      ).toLocaleString()}
                    </span>
                    {view === "trash" && <small>{purgeTiming(item.purgeEligibleAtMs)}</small>}
                    <span aria-hidden="true">→</span>
                  </footer>
                </Link>
              </li>
            ))}
          </ol>
          {error && (
            <p className="capture-card__error" role="alert">
              Could not load more tidbits: {error}
            </p>
          )}
          {cursor && (
            <div className="library-page__more">
              <Button disabled={loadingMore} onClick={loadMore} variant="surface">
                {loadingMore ? "Loading…" : "Load more"}
              </Button>
            </div>
          )}
        </>
      )}
    </main>
  );
}

function LibraryTab({
  label,
  selected,
  view,
}: {
  label: string;
  selected: boolean;
  view: LibraryView;
}) {
  return (
    <Link
      aria-current={selected ? "page" : undefined}
      className={selected ? "library-tabs__active" : undefined}
      search={view === "recent" ? {} : { view }}
      to="/library"
    >
      {label}
    </Link>
  );
}

function purgeTiming(eligibleAtMs: number | null): string {
  if (eligibleAtMs === null) return "Purge date unavailable";
  if (eligibleAtMs <= Date.now()) return "Permanent delete available";
  return `Permanent delete after ${new Date(eligibleAtMs).toLocaleDateString()}`;
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
