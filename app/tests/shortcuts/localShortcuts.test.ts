import { describe, expect, it } from "vitest";
import {
  DEFAULT_LOCAL_KEYBOARD_BINDINGS,
  LocalShortcutCommand,
  keyboardEventMatchesAccelerator,
  noteLinkForLocation,
  noteTargetForDeepLink,
  readLocalKeyboardBindings,
  validateLocalKeyboardBindings,
  writeLocalKeyboardBindings,
} from "../../src/shortcuts/localShortcuts";

describe("local shortcut settings", () => {
  const withCopyNoteLink = (accelerator: string) =>
    DEFAULT_LOCAL_KEYBOARD_BINDINGS.map((binding) =>
      binding.command === LocalShortcutCommand.CopyNoteLink ? { ...binding, accelerator } : binding,
    );

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
    expect(validateLocalKeyboardBindings(withCopyNoteLink("super+KeyF"))).toMatch(/reserved/iu);
    for (const accelerator of [
      "super+KeyA",
      "super+KeyC",
      "super+KeyV",
      "super+KeyX",
      "super+KeyZ",
      "shift+super+KeyZ",
    ]) {
      expect(validateLocalKeyboardBindings(withCopyNoteLink(accelerator))).toMatch(/reserved/iu);
    }
    expect(validateLocalKeyboardBindings(withCopyNoteLink("super+keyc"))).toMatch(/reserved/iu);
  });
});

describe("note links", () => {
  const noteId = "019f547b-6200-7000-8000-000000000001";

  it("emits external app links and strips search state from a clean note link", () => {
    const href = `http://tauri.localhost/?outer=debug#/notes/${noteId}?passage=passage-1&query=cool`;
    expect(noteLinkForLocation(href, true)).toBe(
      `kosh://note/${noteId}?passage=passage-1&query=cool`,
    );
    expect(noteLinkForLocation(href, false)).toBe(`kosh://note/${noteId}`);
    expect(noteLinkForLocation(`http://localhost/#/new/${noteId}`, false)).toBe(
      `kosh://note/${noteId}`,
    );
  });

  it("accepts only canonical Kosh note targets", () => {
    expect(noteTargetForDeepLink(`kosh://note/${noteId}?passage=passage-1`)).toEqual({
      noteId,
      passage: "passage-1",
    });
    expect(noteTargetForDeepLink(`http://tauri.localhost/#/notes/${noteId}`)).toBeNull();
    expect(noteTargetForDeepLink(`kosh://note/${noteId}/extra`)).toBeNull();
    expect(noteTargetForDeepLink(`kosh://note/${noteId}?passage=one&passage=two`)).toBeNull();
    expect(noteTargetForDeepLink(`kosh://note/${noteId}#unexpected`)).toBeNull();
  });
});
