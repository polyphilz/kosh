import { useNavigate } from "@tanstack/react-router";
import { useCallback, useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import type {
  PassageEmbeddingIndexStatus,
  PassageSearchResult,
  SearchPassagesResponse,
  SemanticRuntimeStatus,
} from "../backend/contracts";
import { useBackend } from "../backend/context";
import { Dialog } from "../components/Dialog";
import { checkpointBeforeSearch } from "./checkpoint";
import { HighlightedText } from "./HighlightedText";
import { citationLocation, sourceDisplay } from "./presentation";

const SEARCH_DEBOUNCE_MS = 160;
const SEARCH_RESULT_LIMIT = 24;
export const SEARCH_RESULT_SELECTED_EVENT = "kosh:search-result-selected";

export interface SearchResultSelectedDetail {
  noteId: string;
  passageId: string;
}

interface SearchOverlayProps {
  onClose: () => void;
  onResultOpen: () => void;
  open: boolean;
}

export function SearchOverlay({ onClose, onResultOpen, open }: SearchOverlayProps) {
  const backend = useBackend();
  const navigate = useNavigate();
  const listboxId = useId();
  const request = useRef(0);
  const [query, setQuery] = useState("");
  const [response, setResponse] = useState<SearchPassagesResponse | null>(null);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  const semantic = useSemanticStatus(open);

  useEffect(() => {
    if (open) return;
    request.current += 1;
    setQuery("");
    setResponse(null);
    setSelectedIndex(0);
    setSearching(false);
    setError(null);
    setRetry(0);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const trimmed = query.trim();
    const requestId = ++request.current;
    setResponse(null);
    setError(null);
    setSelectedIndex(0);
    setSearching(trimmed.length > 0);
    if (!trimmed) return;

    const timer = window.setTimeout(() => {
      void checkpointBeforeSearch()
        .then(() => {
          if (request.current !== requestId) return null;
          return backend.searchPassages({
            query: trimmed,
            mode: "DEFAULT",
            limit: SEARCH_RESULT_LIMIT,
          });
        })
        .then((nextResponse) => {
          if (request.current !== requestId || !nextResponse) return;
          setResponse(nextResponse);
        })
        .catch((reason: unknown) => {
          if (request.current !== requestId) return;
          setError(errorMessage(reason));
        })
        .finally(() => {
          if (request.current === requestId) setSearching(false);
        });
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      window.clearTimeout(timer);
      if (request.current === requestId) request.current += 1;
    };
  }, [backend, open, query, retry]);

  const results = response?.results ?? [];
  const selected = results[selectedIndex];
  const openResult = useCallback(
    (result: PassageSearchResult) => {
      void navigate({
        to: "/notes/$noteId",
        params: { noteId: result.note.id },
        search: { passage: result.passageId },
      }).then(() => {
        onResultOpen();
        window.dispatchEvent(
          new CustomEvent<SearchResultSelectedDetail>(SEARCH_RESULT_SELECTED_EVENT, {
            detail: { noteId: result.note.id, passageId: result.passageId },
          }),
        );
      });
    },
    [navigate, onResultOpen],
  );

  const onInputKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === "ArrowDown" && results.length > 0) {
      event.preventDefault();
      setSelectedIndex((current) => (current + 1) % results.length);
    } else if (event.key === "ArrowUp" && results.length > 0) {
      event.preventDefault();
      setSelectedIndex((current) => (current - 1 + results.length) % results.length);
    } else if (event.key === "Enter" && selected) {
      event.preventDefault();
      openResult(selected);
    }
  };

  const queryPresent = query.trim().length > 0;
  return (
    <Dialog
      className="search-overlay"
      description="Find an exact passage in your local notes."
      onClose={onClose}
      open={open}
      title="Search notes"
    >
      <div className="search-overlay__input-wrap">
        <span aria-hidden="true">⌕</span>
        <input
          aria-activedescendant={selected ? resultId(listboxId, selected.passageId) : undefined}
          aria-autocomplete="list"
          aria-controls={listboxId}
          aria-expanded={queryPresent}
          aria-label="Search notes"
          autoComplete="off"
          className="search-overlay__input"
          data-autofocus
          data-kosh-search-input
          maxLength={512}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={onInputKeyDown}
          placeholder="Search a thought, phrase, source, or file…"
          role="combobox"
          spellCheck={false}
          type="search"
          value={query}
        />
        <kbd>ESC</kbd>
      </div>

      <div aria-live="polite" className="search-overlay__status" role="status">
        <span>{searchStatus(queryPresent, searching, response, error)}</span>
        <span>{semanticLabel(response, semantic.runtime, semantic.index, semantic.error)}</span>
      </div>

      <div aria-busy={searching || undefined} className="search-overlay__results">
        {error ? (
          <div className="search-overlay__state" role="alert">
            <strong>Search failed</strong>
            <span>{error}</span>
            <button onClick={() => setRetry((value) => value + 1)} type="button">
              Try again
            </button>
          </div>
        ) : !queryPresent ? (
          <div className="search-overlay__state">
            <strong>Search your notes</strong>
            <span>Results stay on this device and disappear when you close this window.</span>
          </div>
        ) : !searching && response && results.length === 0 ? (
          <div className="search-overlay__state">
            <strong>No passages found</strong>
            <span>Try fewer words, a source domain, filename, or remembered phrase.</span>
          </div>
        ) : (
          <div aria-label="Matching passages" id={listboxId} role="listbox">
            {results.map((result, index) => (
              <SearchOverlayResult
                active={index === selectedIndex}
                id={resultId(listboxId, result.passageId)}
                key={result.passageId}
                onPointerMove={() => setSelectedIndex(index)}
                onSelect={() => openResult(result)}
                result={result}
              />
            ))}
          </div>
        )}
      </div>
    </Dialog>
  );
}

function SearchOverlayResult({
  active,
  id,
  onPointerMove,
  onSelect,
  result,
}: {
  active: boolean;
  id: string;
  onPointerMove: () => void;
  onSelect: () => void;
  result: PassageSearchResult;
}) {
  const attachment = result.citation.attachment;
  const sources = result.citation.sources.slice(0, 2);
  return (
    <button
      aria-selected={active}
      className="search-overlay-result"
      data-active={active || undefined}
      id={id}
      onClick={onSelect}
      onPointerMove={onPointerMove}
      role="option"
      tabIndex={-1}
      type="button"
    >
      <span className="search-overlay-result__header">
        <strong>{result.note.displayTitle}</strong>
        <span>{attachment?.displayFilename ?? citationLocation(result.citation)}</span>
      </span>
      {result.citation.headingContext.length > 0 && (
        <span className="search-overlay-result__heading">
          {result.citation.headingContext.join(" › ")}
        </span>
      )}
      <span className="search-overlay-result__excerpt">
        <HighlightedText
          fields={["BODY", "EXTRACTED_TEXT"]}
          highlights={result.highlights}
          text={result.citation.excerpt}
        />
      </span>
      <span className="search-overlay-result__meta">
        <span>{citationLocation(result.citation)}</span>
        {sources.length > 0 && <span>{sources.map(sourceDisplay).join(" · ")}</span>}
      </span>
    </button>
  );
}

function useSemanticStatus(open: boolean) {
  const backend = useBackend();
  const request = useRef(0);
  const [runtime, setRuntime] = useState<SemanticRuntimeStatus | null>(null);
  const [index, setIndex] = useState<PassageEmbeddingIndexStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    let active = true;
    let timeout: number | undefined;
    const poll = async () => {
      const requestId = ++request.current;
      try {
        const [nextRuntime, nextIndex] = await Promise.all([
          backend.semanticRuntimeStatus(),
          backend.passageEmbeddingIndexStatus(),
        ]);
        if (!active || request.current !== requestId) return;
        setRuntime(nextRuntime);
        setIndex(nextIndex);
        setError(null);
      } catch (reason) {
        if (!active || request.current !== requestId) return;
        setError(errorMessage(reason));
      }
      if (active) timeout = window.setTimeout(() => void poll(), 2_000);
    };
    void poll();
    return () => {
      active = false;
      request.current += 1;
      if (timeout !== undefined) window.clearTimeout(timeout);
    };
  }, [backend, open]);
  return { error, index, runtime };
}

function semanticLabel(
  response: SearchPassagesResponse | null,
  runtime: SemanticRuntimeStatus | null,
  index: PassageEmbeddingIndexStatus | null,
  error: string | null,
): string {
  if (response?.executionMode === "HYBRID") return "Hybrid";
  if (response?.semanticReadiness === "INDEXING" || index?.phase === "INDEXING") {
    return "Lexical · semantic index rebuilding";
  }
  if (response?.semanticReadiness === "FAILED" || runtime?.phase === "FAILED" || error) {
    return "Lexical · semantic unavailable";
  }
  if (runtime?.phase === "READY" && index?.phase === "READY") return "Semantic ready";
  return "Lexical ready";
}

function searchStatus(
  queryPresent: boolean,
  searching: boolean,
  response: SearchPassagesResponse | null,
  error: string | null,
): string {
  if (error) return "Search needs attention";
  if (!queryPresent) return "Type to search";
  if (searching) return "Searching locally…";
  if (!response) return "Waiting to search";
  return `${response.results.length} ${response.results.length === 1 ? "passage" : "passages"}`;
}

function resultId(listboxId: string, passageId: string): string {
  return `${listboxId}-${passageId}`.replace(/[^A-Za-z0-9_-]/gu, "-");
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
