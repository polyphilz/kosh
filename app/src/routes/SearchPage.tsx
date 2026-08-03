import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";
import type {
  CitationResolution,
  PassageEmbeddingIndexStatus,
  PassageSearchResult,
  SearchPassagesResponse,
  SemanticRuntimeStatus,
} from "../backend/contracts";
import { useBackend } from "../backend/context";
import { Button } from "../components/Button";
import { Input } from "../components/Input";
import { EmptyState, ErrorState } from "../components/States";
import { Status } from "../components/Status";
import { CitationDetail } from "../search/CitationDetail";
import { SearchResultCard } from "../search/SearchResultCard";

const SEARCH_DEBOUNCE_MS = 180;
const SEARCH_RESULT_LIMIT = 40;

export function SearchPage() {
  const backend = useBackend();
  const navigate = useNavigate({ from: "/search" });
  const {
    exact: routeExact,
    passage: routePassage,
    q: routeQuery,
  } = useSearch({ from: "/search" });
  const inputRef = useRef<HTMLInputElement>(null);
  const resultRefs = useRef(new Map<string, HTMLButtonElement>());
  const detailRef = useRef<HTMLElement>(null);
  const searchRequest = useRef(0);
  const citationRequest = useRef(0);
  const focusDetailAfterLoad = useRef(false);
  const [query, setQuery] = useState(routeQuery ?? "");
  const [exact, setExact] = useState(routeExact ?? false);
  const [response, setResponse] = useState<SearchPassagesResponse | null>(null);
  const [searchRevision, setSearchRevision] = useState(0);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [citation, setCitation] = useState<CitationResolution | null>(null);
  const [citationLoading, setCitationLoading] = useState(false);
  const [citationError, setCitationError] = useState<string | null>(null);
  const semantic = useSemanticSearchStatus();

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  useEffect(() => {
    setQuery(routeQuery ?? "");
    setExact(routeExact ?? false);
  }, [routeExact, routeQuery]);

  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.metaKey && !event.altKey && !event.ctrlKey && event.key.toLowerCase() === "f") {
        event.preventDefault();
        inputRef.current?.focus();
        inputRef.current?.select();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  useEffect(() => {
    const trimmedQuery = query.trim();
    const requestId = ++searchRequest.current;
    setSearchError(null);
    setResponse(null);
    setSearching(trimmedQuery.length > 0);
    if (!trimmedQuery) {
      return;
    }

    const timer = window.setTimeout(() => {
      void backend
        .searchPassages({
          query: trimmedQuery,
          mode: exact ? "EXACT" : "DEFAULT",
          limit: SEARCH_RESULT_LIMIT,
        })
        .then((nextResponse) => {
          if (searchRequest.current !== requestId) return;
          setResponse(nextResponse);
        })
        .catch((reason: unknown) => {
          if (searchRequest.current !== requestId) return;
          setSearchError(errorMessage(reason));
        })
        .finally(() => {
          if (searchRequest.current === requestId) setSearching(false);
        });
    }, SEARCH_DEBOUNCE_MS);

    return () => {
      window.clearTimeout(timer);
      if (searchRequest.current === requestId) searchRequest.current += 1;
    };
  }, [backend, exact, query, searchRevision]);

  useEffect(() => {
    const requestId = ++citationRequest.current;
    setCitationError(null);
    if (!routePassage) {
      setCitation(null);
      setCitationLoading(false);
      focusDetailAfterLoad.current = false;
      return;
    }
    setCitationLoading(true);
    void backend
      .resolveCitation(routePassage)
      .then((resolved) => {
        if (citationRequest.current !== requestId) return;
        setCitation(resolved);
      })
      .catch((reason: unknown) => {
        if (citationRequest.current !== requestId) return;
        setCitation(null);
        setCitationError(errorMessage(reason));
      })
      .finally(() => {
        if (citationRequest.current !== requestId) return;
        setCitationLoading(false);
        if (focusDetailAfterLoad.current) {
          focusDetailAfterLoad.current = false;
          window.requestAnimationFrame(() => detailRef.current?.focus());
        }
      });
  }, [backend, routePassage]);

  const clearSelection = useCallback(() => {
    if (!routePassage) return;
    void navigate({
      replace: true,
      search: searchRouteState(query, exact),
      to: "/search",
    });
  }, [exact, navigate, query, routePassage]);

  const updateQuery = (value: string) => {
    setQuery(value);
    void navigate({
      replace: true,
      search: searchRouteState(value, exact),
      to: "/search",
    });
  };

  const updateExact = (checked: boolean) => {
    setExact(checked);
    void navigate({
      replace: true,
      search: searchRouteState(query, checked),
      to: "/search",
    });
  };

  const selectResult = (passageId: string, focusDetail = false) => {
    if (focusDetail && passageId === routePassage && citation && !citationLoading) {
      window.requestAnimationFrame(() => detailRef.current?.focus());
      return;
    }
    focusDetailAfterLoad.current = focusDetail;
    void navigate({
      search: searchRouteState(query, exact, passageId),
      to: "/search",
    });
  };

  const results = response?.results ?? [];
  const selectedResult = results.find((result) => result.passageId === routePassage);
  const moveResultFocus = (currentPassageId: string | null, delta: -1 | 1) => {
    if (results.length === 0) return;
    const currentIndex = currentPassageId
      ? results.findIndex((result) => result.passageId === currentPassageId)
      : -1;
    const nextIndex =
      currentIndex < 0
        ? delta > 0
          ? 0
          : results.length - 1
        : Math.max(0, Math.min(results.length - 1, currentIndex + delta));
    const next = results[nextIndex];
    if (!next) return;
    selectResult(next.passageId);
    window.requestAnimationFrame(() => resultRefs.current.get(next.passageId)?.focus());
  };

  const resultKeyDown = (event: KeyboardEvent<HTMLButtonElement>, result: PassageSearchResult) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      moveResultFocus(result.passageId, 1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      moveResultFocus(result.passageId, -1);
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      selectResult(result.passageId, true);
    } else if (event.key === "Escape") {
      event.preventDefault();
      clearSelection();
      inputRef.current?.focus();
    }
  };

  const queryPresent = query.trim().length > 0;
  return (
    <main className="page search-page">
      <header className="page-header search-page__header">
        <div>
          <p className="page-kicker">Your local knowledge</p>
          <h1>Search</h1>
          <p>Start typing to retrieve the exact passage and its provenance.</p>
        </div>
        <Status live tone={semantic.tone}>
          {semantic.shortLabel}
        </Status>
      </header>

      <section aria-label="Search controls" className="search-command">
        <div className="search-command__input">
          <span aria-hidden="true">⌕</span>
          <label className="visually-hidden" htmlFor="library-search">
            Search tidbits
          </label>
          <Input
            autoComplete="off"
            id="library-search"
            maxLength={512}
            onChange={(event) => updateQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing) return;
              if (event.key === "ArrowDown" && results.length > 0) {
                event.preventDefault();
                moveResultFocus(routePassage ?? null, 1);
              } else if (event.key === "Escape" && query) {
                event.preventDefault();
                updateQuery("");
              }
            }}
            placeholder="Search a thought, phrase, or idea…"
            ref={inputRef}
            type="search"
            value={query}
          />
          <kbd>⌘F</kbd>
        </div>
        <label className="search-command__exact">
          <input
            checked={exact}
            onChange={(event) => updateExact(event.target.checked)}
            type="checkbox"
          />
          <span>Exact</span>
        </label>
        <Link className="search-command__research" to="/research">
          Research
          <span aria-hidden="true">↗</span>
        </Link>
      </section>

      <SemanticNotice
        error={semantic.error}
        index={semantic.index}
        onPrepare={semantic.prepare}
        preparing={semantic.preparing}
        response={response}
        runtime={semantic.runtime}
      />

      <div aria-live="polite" className="search-summary" role="status">
        <span>{searchSummary(queryPresent, searching, response, searchError)}</span>
        {response && <span>{response.results.length} passages</span>}
      </div>

      <section aria-label="Search workspace" aria-busy={searching} className="search-workspace">
        <div className="search-results">
          {searchError ? (
            <ErrorState
              action={
                <Button onClick={() => setSearchRevision((value) => value + 1)} size="compact">
                  Try again
                </Button>
              }
              detail={searchError}
              title="Search failed"
            />
          ) : !queryPresent ? (
            <EmptyState
              detail="Use a phrase, identifier, source domain, formula, or half-remembered idea."
              title="Ask your library anything"
            />
          ) : !searching && response && results.length === 0 ? (
            <EmptyState
              detail={
                exact
                  ? "No passage contains every exact term. Turn off Exact to broaden the search."
                  : "Try fewer words, a source name, or Exact for a literal phrase."
              }
              title="No supporting passages"
            />
          ) : (
            <div aria-label="Supporting passages" className="search-result-list" role="listbox">
              {results.map((result) => (
                <SearchResultCard
                  active={result.passageId === routePassage}
                  key={result.passageId}
                  onKeyDown={(event) => resultKeyDown(event, result)}
                  onSelect={() => selectResult(result.passageId)}
                  ref={(element) => {
                    if (element) resultRefs.current.set(result.passageId, element);
                    else resultRefs.current.delete(result.passageId);
                  }}
                  result={result}
                />
              ))}
            </div>
          )}
        </div>

        <div className="search-detail-column">
          <div aria-label="Citation history" className="search-history-controls">
            <Button
              aria-label="Back to previous citation"
              onClick={() => window.history.back()}
              size="icon"
              variant="ghost"
            >
              ←
            </Button>
            <Button
              aria-label="Forward to next citation"
              onClick={() => window.history.forward()}
              size="icon"
              variant="ghost"
            >
              →
            </Button>
            {routePassage && (
              <Button onClick={clearSelection} size="compact" variant="ghost">
                Close passage
              </Button>
            )}
          </div>
          <CitationDetail
            citation={citation}
            error={citationError}
            focusRef={detailRef}
            loading={citationLoading}
            onOpenAttachment={(attachmentId) => backend.openAttachmentExternal(attachmentId)}
            result={selectedResult}
            tidbitOrigin="search"
            tidbitReturnSearch={{ exact: routeExact, q: routeQuery }}
          />
        </div>
      </section>
    </main>
  );
}

interface SemanticNoticeProps {
  error: string | null;
  index: PassageEmbeddingIndexStatus | null;
  onPrepare: () => void;
  preparing: boolean;
  response: SearchPassagesResponse | null;
  runtime: SemanticRuntimeStatus | null;
}

function SemanticNotice({
  error,
  index,
  onPrepare,
  preparing,
  response,
  runtime,
}: SemanticNoticeProps) {
  const label = semanticNoticeLabel(runtime, index, response);
  const canPrepare =
    !preparing &&
    (runtime?.phase === "NOT_DOWNLOADED" ||
      runtime?.phase === "VERIFICATION_REQUIRED" ||
      runtime?.phase === "FAILED");
  return (
    <section
      aria-label="Semantic search status"
      className="semantic-notice"
      role={error ? "alert" : "status"}
    >
      <span aria-hidden="true" className="semantic-notice__pulse" />
      <span>{error ?? label}</span>
      {canPrepare && (
        <Button onClick={onPrepare} size="compact" variant="ghost">
          Enable semantic
        </Button>
      )}
      {preparing && <span>Preparing…</span>}
    </section>
  );
}

function useSemanticSearchStatus() {
  const backend = useBackend();
  const active = useRef(false);
  const statusRequest = useRef(0);
  const [runtime, setRuntime] = useState<SemanticRuntimeStatus | null>(null);
  const [index, setIndex] = useState<PassageEmbeddingIndexStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [preparing, setPreparing] = useState(false);

  const refresh = useCallback(async () => {
    const requestId = ++statusRequest.current;
    try {
      const [nextRuntime, nextIndex] = await Promise.all([
        backend.semanticRuntimeStatus(),
        backend.passageEmbeddingIndexStatus(),
      ]);
      if (!active.current || statusRequest.current !== requestId) return;
      setRuntime(nextRuntime);
      setIndex(nextIndex);
      setError(null);
    } catch (reason) {
      if (!active.current || statusRequest.current !== requestId) return;
      setError(`Semantic status unavailable: ${errorMessage(reason)}`);
    }
  }, [backend]);

  useEffect(() => {
    active.current = true;
    let cancelled = false;
    let timeoutId: number | undefined;
    const poll = async () => {
      await refresh();
      if (!cancelled) {
        timeoutId = window.setTimeout(() => void poll(), 1_500);
      }
    };
    void poll();
    return () => {
      cancelled = true;
      if (timeoutId !== undefined) window.clearTimeout(timeoutId);
      active.current = false;
      statusRequest.current += 1;
    };
  }, [refresh]);

  const prepare = () => {
    if (preparing) return;
    setPreparing(true);
    setError(null);
    void backend
      .prepareSemanticRuntime()
      .then(() => refresh())
      .catch((reason: unknown) => {
        if (!active.current) return;
        setError(`Could not prepare semantic search: ${errorMessage(reason)}`);
      })
      .finally(() => {
        if (active.current) setPreparing(false);
      });
  };

  const tone = error
    ? ("danger" as const)
    : runtime?.phase === "READY" && index?.phase === "READY"
      ? ("success" as const)
      : runtime?.phase === "FAILED" || index?.phase === "FAILED"
        ? ("danger" as const)
        : ("warning" as const);
  return {
    error,
    index,
    prepare,
    preparing,
    runtime,
    shortLabel:
      runtime?.phase === "READY" && index?.phase === "READY"
        ? "Semantic ready"
        : "Lexical search ready",
    tone,
  };
}

function searchSummary(
  queryPresent: boolean,
  searching: boolean,
  response: SearchPassagesResponse | null,
  error: string | null,
): string {
  if (error) return "Search needs attention";
  if (!queryPresent) return "Ready";
  if (searching) return "Searching locally…";
  if (!response) return "Waiting to search";
  switch (response.executionMode) {
    case "EXACT":
      return "Exact lexical matches";
    case "HYBRID":
      return "Hybrid matches";
    case "LEXICAL_ONLY":
      return "Lexical matches";
    default:
      return response.executionMode satisfies never;
  }
}

function semanticNoticeLabel(
  runtime: SemanticRuntimeStatus | null,
  index: PassageEmbeddingIndexStatus | null,
  response: SearchPassagesResponse | null,
): string {
  if (response?.executionMode === "HYBRID") return "Semantic and lexical retrieval active";
  if (response?.semanticReadiness === "NOT_REQUESTED") {
    return "Exact mode uses lexical retrieval by design";
  }
  if (response?.semanticReadiness === "INDEXING" || index?.phase === "INDEXING") {
    const indexed = index ? `${index.indexedPassages}/${index.totalPassages}` : "";
    return `Indexing passages${indexed ? ` · ${indexed}` : ""} · lexical search stays active`;
  }
  if (response?.semanticReadiness === "FAILED" || runtime?.phase === "FAILED") {
    return "Semantic search needs attention · lexical search stays active";
  }
  if (runtime?.phase === "DOWNLOADING") {
    const percent =
      runtime.modelBytes > 0
        ? Math.min(100, Math.floor((runtime.downloadedBytes / runtime.modelBytes) * 100))
        : 0;
    return `Downloading semantic model · ${percent}% · lexical search stays active`;
  }
  if (
    runtime?.phase === "VERIFYING" ||
    runtime?.phase === "STARTING" ||
    runtime?.phase === "VERIFICATION_REQUIRED"
  ) {
    return "Preparing semantic search · lexical search stays active";
  }
  if (runtime?.phase === "READY" && index?.phase === "READY") {
    return response?.executionMode === "LEXICAL_ONLY"
      ? "Semantic index ready · this result used lexical retrieval"
      : "Semantic search ready";
  }
  return "Semantic search is off · lexical search still works";
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

function searchRouteState(query: string, exact: boolean, passage?: string) {
  return {
    ...(query.trim() ? { q: query } : {}),
    ...(exact ? { exact: true as const } : {}),
    ...(passage ? { passage } : {}),
  };
}
