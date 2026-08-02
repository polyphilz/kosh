import { describe, expect, it } from "vitest";
import { createFixtureFactory } from "../../src/testing/fixtures";

describe("fixture factory", () => {
  it("builds deterministic fixtures for every planned owner type", () => {
    const fixtures = createFixtureFactory();

    expect({
      tidbit: fixtures.tidbit(),
      revision: fixtures.tidbitRevision(),
      source: fixtures.source(),
      attachment: fixtures.attachment(),
      passage: fixtures.passage(),
    }).toMatchInlineSnapshot(`
      {
        "attachment": {
          "byteLength": 1024,
          "extractionState": "ready",
          "filename": "reference.pdf",
          "id": "attachment-4",
          "mediaType": "application/pdf",
        },
        "passage": {
          "content": "A citation-sized fixture passage.",
          "id": "passage-5",
          "locator": {
            "endBlock": 0,
            "kind": "markdown-blocks",
            "startBlock": 0,
          },
          "revisionId": "revision-1",
        },
        "revision": {
          "bodyMarkdown": "Fixture body",
          "createdAtMs": 1785201600000,
          "id": "revision-2",
          "tidbitId": "tidbit-1",
          "title": "Fixture tidbit",
        },
        "source": {
          "id": "source-3",
          "label": "Fixture source",
          "url": "https://example.com/source",
        },
        "tidbit": {
          "createdAtMs": 1785201600000,
          "currentRevisionId": "revision-1",
          "deletedAtMs": null,
          "id": "tidbit-1",
          "updatedAtMs": 1785201600000,
        },
      }
    `);
  });
});
