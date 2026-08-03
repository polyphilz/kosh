export const LocalShortcutCommand = {
  CopyNoteLink: "COPY_NOTE_LINK",
  CopyExactNoteLink: "COPY_EXACT_NOTE_LINK",
} as const;

export type LocalShortcutCommand = (typeof LocalShortcutCommand)[keyof typeof LocalShortcutCommand];

export interface LocalKeyboardBinding {
  command: LocalShortcutCommand;
  accelerator: string;
}

export const DEFAULT_COPY_NOTE_LINK_ACCELERATOR = "super+KeyL";
export const DEFAULT_COPY_EXACT_NOTE_LINK_ACCELERATOR = "shift+super+KeyL";

export const DEFAULT_LOCAL_KEYBOARD_BINDINGS: readonly LocalKeyboardBinding[] = [
  {
    command: LocalShortcutCommand.CopyNoteLink,
    accelerator: DEFAULT_COPY_NOTE_LINK_ACCELERATOR,
  },
  {
    command: LocalShortcutCommand.CopyExactNoteLink,
    accelerator: DEFAULT_COPY_EXACT_NOTE_LINK_ACCELERATOR,
  },
];

const STORAGE_KEY = "kosh.local-shortcuts.v1";
const reservedAccelerators = new Set([
  "super+BracketLeft",
  "super+BracketRight",
  "super+Comma",
  "super+KeyB",
  "super+KeyF",
  "super+KeyK",
  "super+KeyN",
  "super+Slash",
]);

export function readLocalKeyboardBindings(
  storage: Pick<Storage, "getItem"> = window.localStorage,
): LocalKeyboardBinding[] {
  try {
    const raw = storage.getItem(STORAGE_KEY);
    if (!raw) return cloneDefaultBindings();
    const parsed: unknown = JSON.parse(raw);
    if (!isCompleteBindingSet(parsed) || validateLocalKeyboardBindings(parsed) !== null) {
      return cloneDefaultBindings();
    }
    return parsed.map((binding) => ({ ...binding }));
  } catch {
    return cloneDefaultBindings();
  }
}

export function writeLocalKeyboardBindings(
  bindings: readonly LocalKeyboardBinding[],
  storage: Pick<Storage, "setItem"> = window.localStorage,
): void {
  if (!isCompleteBindingSet(bindings)) {
    throw new Error("Local shortcuts must contain every app command exactly once.");
  }
  storage.setItem(STORAGE_KEY, JSON.stringify(bindings));
}

export function validateLocalKeyboardBindings(
  bindings: readonly LocalKeyboardBinding[],
  globalAccelerators: readonly string[] = [],
): string | null {
  if (!isCompleteBindingSet(bindings)) {
    return "Local shortcuts must contain every app command exactly once.";
  }
  const accelerators = bindings.map((binding) => binding.accelerator.toLowerCase());
  if (new Set(accelerators).size !== accelerators.length) {
    return "Two Kosh commands cannot use the same shortcut.";
  }
  const globals = new Set(globalAccelerators.map((accelerator) => accelerator.toLowerCase()));
  if (accelerators.some((accelerator) => globals.has(accelerator))) {
    return "That shortcut is already used by a global Kosh command.";
  }
  if (bindings.some((binding) => reservedAccelerators.has(binding.accelerator))) {
    return "That shortcut is reserved by another Kosh command.";
  }
  return null;
}

export function localBindingFor(
  bindings: readonly LocalKeyboardBinding[],
  command: LocalShortcutCommand,
): LocalKeyboardBinding | undefined {
  return bindings.find((binding) => binding.command === command);
}

export function keyboardEventMatchesAccelerator(
  event: Pick<KeyboardEvent, "altKey" | "code" | "ctrlKey" | "metaKey" | "shiftKey">,
  accelerator: string,
): boolean {
  const expected = accelerator.split("+");
  const code = expected.pop();
  if (!code || event.code !== code) return false;
  const modifiers = new Set(expected);
  return (
    event.altKey === modifiers.has("alt") &&
    event.ctrlKey === modifiers.has("control") &&
    event.metaKey === modifiers.has("super") &&
    event.shiftKey === modifiers.has("shift")
  );
}

export function noteLinkForLocation(href: string, exact: boolean): string {
  if (exact) return href;
  const url = new URL(href);
  url.search = "";
  const queryIndex = url.hash.indexOf("?");
  if (queryIndex >= 0) url.hash = url.hash.slice(0, queryIndex);
  return url.href;
}

function cloneDefaultBindings(): LocalKeyboardBinding[] {
  return DEFAULT_LOCAL_KEYBOARD_BINDINGS.map((binding) => ({ ...binding }));
}

function isCompleteBindingSet(value: unknown): value is LocalKeyboardBinding[] {
  if (!Array.isArray(value) || value.length !== DEFAULT_LOCAL_KEYBOARD_BINDINGS.length) {
    return false;
  }
  const commands = new Set<LocalShortcutCommand>();
  for (const binding of value) {
    if (!isRecord(binding)) return false;
    if (!Object.values(LocalShortcutCommand).includes(binding.command as LocalShortcutCommand)) {
      return false;
    }
    if (typeof binding.accelerator !== "string" || !isAccelerator(binding.accelerator)) {
      return false;
    }
    commands.add(binding.command as LocalShortcutCommand);
  }
  return commands.size === DEFAULT_LOCAL_KEYBOARD_BINDINGS.length;
}

function isAccelerator(value: string): boolean {
  const parts = value.split("+");
  const code = parts.pop();
  if (!code || parts.length === 0 || !/^[A-Za-z][A-Za-z0-9]*$/u.test(code)) return false;
  const modifiers = new Set(parts);
  return (
    modifiers.size === parts.length &&
    parts.every((part) => ["alt", "control", "shift", "super"].includes(part))
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
