import { describe, expect, it } from "vitest";
import {
  createEmptyKoshDocument,
  createKoshDocumentFromMarkdown,
  parseKoshDocument,
  serializeKoshDocument,
} from "../../src/editor/document";

describe("Kosh document JSON", () => {
  it("persists stable IDs through serialization", () => {
    const documentJson = createKoshDocumentFromMarkdown("one\n\n- two\n  - three");
    const first = parseKoshDocument(documentJson);
    const second = parseKoshDocument(serializeKoshDocument(first));

    expect(second).toEqual(first);
    expect(first[0]?.id).toBeTruthy();
    expect(first[1]?.children?.[0]?.id).toBeTruthy();
  });

  it("creates a valid empty editor document", () => {
    expect(parseKoshDocument(createEmptyKoshDocument())).toMatchObject([{ type: "paragraph" }]);
  });

  it("rejects duplicate IDs at any depth", () => {
    expect(() =>
      parseKoshDocument(
        JSON.stringify({
          schemaVersion: 1,
          blocks: [
            {
              id: "same",
              type: "paragraph",
              children: [{ id: "same", type: "paragraph" }],
            },
          ],
        }),
      ),
    ).toThrow("duplicated");
  });

  it("rejects blocks outside the Kosh editor schema", () => {
    expect(() =>
      parseKoshDocument(
        JSON.stringify({
          schemaVersion: 1,
          blocks: [{ id: "unsupported", type: "table" }],
        }),
      ),
    ).toThrow("unsupported");
  });
});
