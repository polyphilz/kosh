import { describe, expect, it } from "vitest";
import { isBlockNoteCapability } from "../../src/redesign/spike/bridge";

describe("BlockNote feasibility oracle", () => {
  it("rejects a plain contenteditable-shaped approximation", () => {
    expect(
      isBlockNoteCapability({
        contentEditable: true,
        innerHTML: "<p>looks like an editor</p>",
        snapshot: () => ({ blocks: [], focused: true, selectedBlockIds: [] }),
      }),
    ).toBe(false);
  });

  it("accepts only the explicit BlockNote capability", () => {
    expect(
      isBlockNoteCapability({
        capability: "blocknote",
        snapshot: () => ({ blocks: [], focused: true, selectedBlockIds: [] }),
      }),
    ).toBe(true);
  });
});
