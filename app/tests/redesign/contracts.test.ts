import { describe, expect, it } from "vitest";
import {
  REDESIGN_COMMAND_CONTRACT,
  REDESIGN_NAVIGATION_CONTRACT,
  REDESIGN_ROUTE_CONTRACT,
  REDESIGN_ROUTE_LIMITS,
} from "../../src/redesign/contracts";

describe("note-first redesign contracts", () => {
  it("freezes the bounded ephemeral, durable note, and settings routes", () => {
    expect(REDESIGN_ROUTE_CONTRACT).toEqual({
      coldLaunch: "/new/$draftId",
      newNote: "/new/$draftId",
      note: "/notes/$noteId",
      settings: "/settings",
    });
    expect(REDESIGN_ROUTE_LIMITS).toEqual({
      draftIdCharacters: 36,
      noteIdCharacters: 36,
      passageIdCharacters: 256,
      revisionIdCharacters: 64,
    });
  });

  it("reserves the approved macOS commands without stealing editor bold", () => {
    expect(REDESIGN_COMMAND_CONTRACT).toEqual({
      newNote: { id: "new-note", macosAccelerator: "CommandOrControl+N" },
      search: { id: "search", macosAccelerator: "CommandOrControl+K" },
      toggleSidebar: { id: "toggle-sidebar", macosAccelerator: "CommandOrControl+/" },
      settings: { id: "settings", macosAccelerator: "CommandOrControl+," },
    });
    expect(Object.values(REDESIGN_COMMAND_CONTRACT)).not.toContainEqual(
      expect.objectContaining({ macosAccelerator: "CommandOrControl+B" }),
    );
  });

  it("adds history only for user navigation and never for persistence", () => {
    expect(REDESIGN_NAVIGATION_CONTRACT).toEqual({
      coldLaunch: "replace",
      firstCheckpoint: "replace",
      newNote: "push",
      searchSelection: "push",
      settings: "push",
      autosave: "none",
      checkpoint: "none",
    });
  });
});
