import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { PassageSearchResult } from "../../src/backend/contracts";
import { SearchResultCard } from "../../src/search/SearchResultCard";

describe("SearchResultCard", () => {
  it("highlights a matching attachment filename even when the excerpt did not match", () => {
    const result: PassageSearchResult = {
      passageId: "attachment-passage",
      score: 1,
      matchedFields: ["ATTACHMENT_NAME"],
      highlights: [
        {
          field: "ATTACHMENT_NAME",
          startChar: 0,
          endChar: 11,
        },
      ],
      citation: {
        passageId: "attachment-passage",
        excerpt: "Evidence on an unrelated page.",
        headingContext: [],
        constructionVersion: "attachment-v1",
        state: "CURRENT",
        locator: { kind: "PDF_PAGE", page: 4 },
        tidbit: null,
        attachment: {
          id: "attachment-1",
          extractionId: "extraction-1",
          displayFilename: "field-notes.pdf",
          mediaType: "application/pdf",
          deleted: false,
        },
        sources: [],
      },
    };

    render(
      <SearchResultCard active={false} onKeyDown={vi.fn()} onSelect={vi.fn()} result={result} />,
    );

    expect(screen.getByRole("option").querySelector("mark")).toHaveTextContent("field-notes");
  });
});
