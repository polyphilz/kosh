import { describe, expect, it } from "vitest";
import {
  DEFAULT_LOCAL_KEYBOARD_BINDINGS,
  LocalShortcutCommand,
  keyboardEventMatchesAccelerator,
  noteLinkForLocation,
  readLocalKeyboardBindings,
  validateLocalKeyboardBindings,
  writeLocalKeyboardBindings,
} from "../../src/shortcuts/localShortcuts";

describe("local shortcut settings", () => {
  it("persists complete bindings and falls back from malformed storage", () => {
    let stored: string | null = null;
    const storage = {
      getItem: () => stored,
      setItem: (_key: string, value: string) => {
        stored = value;
      },
    };
    const bindings = DEFAULT_LOCAL_KEYBOARD_BINDINGS.map((binding) => ({ ...binding }));
    bindings[0]!.accelerator = "shift+super+KeyC";
    writeLocalKeyboardBindings(bindings, storage);
    expect(readLocalKeyboardBindings(storage)).toEqual(bindings);

    stored = '{"not":"bindings"}';
    expect(readLocalKeyboardBindings(storage)).toEqual(DEFAULT_LOCAL_KEYBOARD_BINDINGS);
  });

  it("matches exact modifiers and rejects conflicts", () => {
    expect(
      keyboardEventMatchesAccelerator(
        { altKey: false, code: "KeyL", ctrlKey: false, metaKey: true, shiftKey: false },
        "super+KeyL",
      ),
    ).toBe(true);
    expect(
      keyboardEventMatchesAccelerator(
        { altKey: false, code: "KeyL", ctrlKey: false, metaKey: true, shiftKey: true },
        "super+KeyL",
      ),
    ).toBe(false);

    const duplicate = DEFAULT_LOCAL_KEYBOARD_BINDINGS.map((binding) => ({ ...binding }));
    duplicate[1]!.accelerator = duplicate[0]!.accelerator;
    expect(validateLocalKeyboardBindings(duplicate)).toMatch(/same shortcut/iu);
    expect(validateLocalKeyboardBindings(DEFAULT_LOCAL_KEYBOARD_BINDINGS, ["super+KeyL"])).toMatch(
      /global Kosh command/iu,
    );
    expect(
      validateLocalKeyboardBindings([
        { command: LocalShortcutCommand.CopyNoteLink, accelerator: "super+KeyF" },
        {
          command: LocalShortcutCommand.CopyExactNoteLink,
          accelerator: "shift+super+KeyL",
        },
      ]),
    ).toMatch(/reserved/iu);
  });
});

describe("note links", () => {
  it("keeps the exact URL and strips all search state from a clean note link", () => {
    const href = "http://tauri.localhost/?outer=debug#/notes/019f?passage=passage-1&query=cool";
    expect(noteLinkForLocation(href, true)).toBe(href);
    expect(noteLinkForLocation(href, false)).toBe("http://tauri.localhost/#/notes/019f");
  });
});
