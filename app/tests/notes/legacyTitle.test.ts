import { describe, expect, it } from "vitest";
import { projectLegacyTitle } from "../../src/notes/legacyTitle";

describe("legacy title projection", () => {
  it("projects a historical title as one escaped leading heading", () => {
    expect(projectLegacyTitle("  Arrays *and* shapes\n", "Body text")).toBe(
      "# Arrays \\*and\\* shapes\n\nBody text",
    );
  });

  it("leaves titleless and blank-title notes unchanged", () => {
    expect(projectLegacyTitle(null, "Body text")).toBe("Body text");
    expect(projectLegacyTitle(" \n ", "Body text")).toBe("Body text");
  });
});
