import { forwardRef, type KeyboardEvent } from "react";
import type { PassageSearchResult, SearchField } from "../backend/contracts";
import { classNames } from "../lib/classNames";
import { HighlightedText } from "./HighlightedText";
import { citationLocation, citationRevision, resultTitle, sourceDisplay } from "./presentation";

interface SearchResultCardProps {
  active: boolean;
  onKeyDown: (event: KeyboardEvent<HTMLButtonElement>) => void;
  onSelect: () => void;
  result: PassageSearchResult;
}

export const SearchResultCard = forwardRef<HTMLButtonElement, SearchResultCardProps>(
  function SearchResultCard({ active, onKeyDown, onSelect, result }, ref) {
    const sources = result.citation.sources.slice(0, 2);
    const titleHighlightFields: SearchField[] = result.citation.tidbit
      ? ["TITLE"]
      : ["ATTACHMENT_NAME"];
    return (
      <button
        aria-controls="search-citation-detail"
        aria-selected={active}
        className={classNames("search-result-card", active && "search-result-card--active")}
        onClick={onSelect}
        onKeyDown={onKeyDown}
        ref={ref}
        role="option"
        type="button"
      >
        <span className="search-result-card__topline">
          <strong>
            <HighlightedText
              fields={titleHighlightFields}
              highlights={result.highlights}
              text={resultTitle(result)}
            />
          </strong>
          <span>{citationRevision(result.citation)}</span>
        </span>
        {result.citation.headingContext.length > 0 && (
          <span className="search-result-card__heading">
            {result.citation.headingContext.join(" › ")}
          </span>
        )}
        <span className="search-result-card__excerpt">
          <HighlightedText
            fields={["BODY", "EXTRACTED_TEXT"]}
            highlights={result.highlights}
            text={result.citation.excerpt}
          />
        </span>
        <span className="search-result-card__meta">
          <span>{citationLocation(result.citation)}</span>
          {sources.length > 0 && <span>{sources.map(sourceDisplay).join(" · ")}</span>}
        </span>
      </button>
    );
  },
);
