import { render } from "@testing-library/react";
import { createRef } from "react";
import { describe, expect, it } from "vitest";
import type { CitationResolution } from "../../src/backend/contracts";
import { CitationDetail } from "../../src/search/CitationDetail";

const attachmentId = "01980c8e-6c00-7000-8000-000000000251";

describe("image citation detail", () => {
  it("renders the immutable image preview with the exact Vision region overlay", () => {
    const citation = imageCitation({
      coordinateSystem: "vision-normalized-bottom-left",
      height: 0.375,
      width: 0.5,
      x: 0.125,
      y: 0.25,
    });
    const { getByLabelText, getByRole, getByText } = render(
      <CitationDetail
        citation={citation}
        error={null}
        focusRef={createRef<HTMLElement>()}
        loading={false}
        result={undefined}
      />,
    );

    expect(getByRole("img", { name: "Cited image: whiteboard.png" })).toHaveAttribute(
      "src",
      `kosh-media://localhost/attachment/${attachmentId}`,
    );
    expect(getByLabelText("Cited image region")).toHaveStyle({
      height: "37.5%",
      left: "12.5%",
      top: "37.5%",
      width: "50%",
    });
    expect(getByText("Highlighted OCR evidence")).toBeInTheDocument();
  });

  it("falls back to citing the whole image when region metadata is unavailable", () => {
    const citation = imageCitation({ height: 2, width: 1, x: 0, y: 0 });
    const { getByText, queryByLabelText } = render(
      <CitationDetail
        citation={citation}
        error={null}
        focusRef={createRef<HTMLElement>()}
        loading={false}
        result={undefined}
      />,
    );

    expect(queryByLabelText("Cited image region")).toBeNull();
    expect(getByText("OCR evidence from the full image")).toBeInTheDocument();
  });
});

function imageCitation(region: unknown): CitationResolution {
  return {
    attachment: {
      deleted: false,
      displayFilename: "whiteboard.png",
      extractionId: "01980c8e-6c00-7000-8000-000000000252",
      id: attachmentId,
      mediaType: "image/png",
    },
    constructionVersion: "ocr-region-v1",
    excerpt: "Exact OCR evidence",
    headingContext: [],
    locator: { kind: "OCR_REGION", page: null, region },
    passageId: "01980c8e-6c00-7000-8000-000000000253",
    sources: [],
    state: "CURRENT",
    tidbit: null,
  };
}
