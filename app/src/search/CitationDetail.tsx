import { Link } from "@tanstack/react-router";
import { useEffect, useState, type RefObject } from "react";
import type { CitationResolution, PassageSearchResult } from "../backend/contracts";
import { Button } from "../components/Button";
import { ErrorState, LoadingState } from "../components/States";
import { Status } from "../components/Status";
import { attachmentMediaUrl } from "../media/gateway";
import { HighlightedText } from "./HighlightedText";
import {
  citationCopyText,
  citationLocation,
  citationOwner,
  citationRevision,
  resultHighlights,
  sourceDisplay,
} from "./presentation";

interface CitationDetailProps {
  citation: CitationResolution | null;
  error: string | null;
  focusRef: RefObject<HTMLElement | null>;
  loading: boolean;
  onOpenAttachment?: (attachmentId: string) => Promise<void>;
  result: PassageSearchResult | undefined;
}

export function CitationDetail({
  citation,
  error,
  focusRef,
  loading,
  onOpenAttachment,
  result,
}: CitationDetailProps) {
  const [copyState, setCopyState] = useState<"IDLE" | "COPIED" | "FAILED">("IDLE");

  useEffect(() => {
    setCopyState("IDLE");
  }, [citation?.passageId]);

  if (loading) {
    return (
      <aside
        aria-label="Citation detail"
        className="search-citation-detail"
        id="search-citation-detail"
      >
        <LoadingState detail="Resolving immutable passage provenance…" title="Opening citation" />
      </aside>
    );
  }
  if (error) {
    return (
      <aside
        aria-label="Citation detail"
        className="search-citation-detail"
        id="search-citation-detail"
      >
        <ErrorState detail={error} title="Could not resolve citation" />
      </aside>
    );
  }
  if (!citation) {
    return (
      <aside
        aria-label="Citation detail"
        className="search-citation-detail search-citation-detail--empty"
        id="search-citation-detail"
      >
        <span aria-hidden="true">↳</span>
        <h2>Open a passage</h2>
        <p>Select a result to inspect its exact text and provenance without leaving search.</p>
      </aside>
    );
  }

  const highlights = resultHighlights(result);
  return (
    <aside
      aria-labelledby="citation-detail-title"
      className="search-citation-detail"
      id="search-citation-detail"
      ref={focusRef}
      tabIndex={-1}
    >
      <header className="search-citation-detail__header">
        <div>
          <p className="page-kicker">Resolved citation</p>
          <h2 id="citation-detail-title">{citationOwner(citation)}</h2>
        </div>
        <Status tone={citation.state === "CURRENT" ? "success" : "warning"}>
          {citation.state === "CURRENT" ? "Current passage" : "Historical passage"}
        </Status>
      </header>
      {citation.state === "HISTORICAL" && (
        <p className="search-citation-detail__notice" role="status">
          This result points to an older immutable revision. The excerpt below is preserved exactly
          as cited.
        </p>
      )}
      <p className="search-citation-detail__location">
        {citationLocation(citation)} · {citationRevision(citation)}
      </p>
      <blockquote className="search-citation-detail__excerpt">
        <HighlightedText
          fields={["BODY", "EXTRACTED_TEXT"]}
          highlights={highlights}
          text={citation.excerpt}
        />
      </blockquote>
      <ImageRegionEvidence citation={citation} />
      <PdfPageEvidence citation={citation} />
      {citation.sources.length > 0 && (
        <section aria-labelledby="citation-sources" className="search-citation-detail__sources">
          <h3 id="citation-sources">Sources</h3>
          <ul>
            {citation.sources.map((source) => (
              <li key={source.id}>
                <span>{sourceDisplay(source)}</span>
                {source.url && <small>{source.url}</small>}
              </li>
            ))}
          </ul>
        </section>
      )}
      <footer className="search-citation-detail__actions">
        <Button
          onClick={() => {
            setCopyState("IDLE");
            const clipboard = navigator.clipboard;
            if (!clipboard) {
              setCopyState("FAILED");
              return;
            }
            void clipboard
              .writeText(citationCopyText(citation))
              .then(() => setCopyState("COPIED"))
              .catch(() => setCopyState("FAILED"));
          }}
          size="compact"
          variant="surface"
        >
          Copy citation
        </Button>
        {citation.tidbit && !citation.tidbit.deleted && (
          <Link
            className="search-citation-detail__link"
            params={{ tidbitId: citation.tidbit.id }}
            search={{ passage: citation.passageId }}
            to="/tidbits/$tidbitId"
          >
            Open tidbit at passage
          </Link>
        )}
        {citation.attachment && (
          <span className="search-citation-detail__attachment">
            {citation.attachment.displayFilename} · {citationLocation(citation)}
          </span>
        )}
        {citation.attachment && citation.locator.kind === "TEXT_LINES" && onOpenAttachment && (
          <Button
            onClick={() => {
              void onOpenAttachment(citation.attachment!.id).catch((reason: unknown) => {
                console.error("Could not open cited attachment", reason);
              });
            }}
            size="compact"
            variant="ghost"
          >
            Open attachment
          </Button>
        )}
      </footer>
      <p
        aria-live="polite"
        className="search-citation-detail__copy-status"
        role={copyState === "FAILED" ? "alert" : "status"}
      >
        {copyState === "COPIED"
          ? "Citation copied"
          : copyState === "FAILED"
            ? "Clipboard access is unavailable"
            : ""}
      </p>
    </aside>
  );
}

function ImageRegionEvidence({ citation }: { citation: CitationResolution }) {
  if (
    citation.locator.kind !== "OCR_REGION" ||
    !citation.attachment?.mediaType.startsWith("image/")
  ) {
    return null;
  }
  const region = normalizedRegion(citation.locator.region);
  return (
    <figure className="search-citation-detail__image">
      <div>
        <img
          alt={`Cited image: ${citation.attachment.displayFilename}`}
          src={attachmentMediaUrl(citation.attachment.id)}
        />
        {region && (
          <span
            aria-label="Cited image region"
            className="search-citation-detail__image-region"
            style={{
              height: `${region.height * 100}%`,
              left: `${region.x * 100}%`,
              top: `${(1 - region.y - region.height) * 100}%`,
              width: `${region.width * 100}%`,
            }}
          />
        )}
      </div>
      <figcaption>
        {region ? "Highlighted OCR evidence" : "OCR evidence from the full image"}
      </figcaption>
    </figure>
  );
}

function PdfPageEvidence({ citation }: { citation: CitationResolution }) {
  if (
    citation.locator.kind !== "PDF_PAGE" ||
    citation.attachment?.mediaType !== "application/pdf"
  ) {
    return null;
  }
  const url = `${attachmentMediaUrl(citation.attachment.id)}#page=${citation.locator.page}`;
  return (
    <figure className="search-citation-detail__pdf">
      <object
        aria-label={`Cited PDF page ${citation.locator.page}`}
        data={url}
        type="application/pdf"
      >
        <span>Inline PDF preview unavailable</span>
      </object>
      <figcaption>
        {citation.attachment.displayFilename} · page {citation.locator.page}
      </figcaption>
    </figure>
  );
}

function normalizedRegion(
  value: unknown,
): { height: number; width: number; x: number; y: number } | null {
  if (!value || typeof value !== "object") {
    return null;
  }
  const region = value as Record<string, unknown>;
  if (region.coordinateSystem !== "vision-normalized-bottom-left") {
    return null;
  }
  const [x, y, width, height] = [region.x, region.y, region.width, region.height];
  if (
    ![x, y, width, height].every(
      (coordinate) => typeof coordinate === "number" && Number.isFinite(coordinate),
    )
  ) {
    return null;
  }
  const numeric = {
    height: height as number,
    width: width as number,
    x: x as number,
    y: y as number,
  };
  return numeric.x >= 0 &&
    numeric.y >= 0 &&
    numeric.width > 0 &&
    numeric.height > 0 &&
    numeric.x + numeric.width <= 1.000_001 &&
    numeric.y + numeric.height <= 1.000_001
    ? numeric
    : null;
}
